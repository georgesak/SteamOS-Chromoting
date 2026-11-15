#include <errno.h>
#include <fcntl.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include <pipewire/pipewire.h>
#include <spa/param/props.h>
#include <spa/param/video/format-utils.h>
#include <spa/pod/builder.h>

typedef void (*pw_frame_cb_t)(
    void *userdata,
    uint32_t width,
    uint32_t height,
    uint32_t stride,
    uint32_t spa_format,
    const uint8_t *data,
    size_t size
);

struct capture_ctx {
    struct pw_main_loop *loop;
    struct pw_context *context;
    struct pw_core *core;
    struct pw_stream *stream;
    struct spa_hook stream_listener;
    struct spa_hook core_listener;
    struct spa_video_info format;
    bool has_format;
    bool was_streaming;
    bool quit_requested;
    int error_code;
    const volatile uint8_t *stop_flag;
    pw_frame_cb_t on_frame;
    void *userdata;
};

static void on_core_error(
    void *data,
    uint32_t id,
    int seq,
    int res,
    const char *message
) {
    struct capture_ctx *ctx = (struct capture_ctx *)data;
    fprintf(stderr, "[pw_capture] core error id=%u res=%d: %s\n", id, res, message ? message : "");
    if (id == PW_ID_CORE) {
        ctx->error_code = res ? res : -EIO;
        ctx->quit_requested = true;
        pw_main_loop_quit(ctx->loop);
    }
}

static const struct pw_core_events core_events = {
    PW_VERSION_CORE_EVENTS,
    .error = on_core_error,
};

static void on_stream_state_changed(
    void *data,
    enum pw_stream_state old,
    enum pw_stream_state state,
    const char *error
) {
    struct capture_ctx *ctx = (struct capture_ctx *)data;
    if (state == PW_STREAM_STATE_STREAMING) {
        ctx->was_streaming = true;
    } else if (state == PW_STREAM_STATE_ERROR) {
        fprintf(stderr, "[pw_capture] stream error: %s\n", error ? error : "unknown");
        ctx->error_code = -EIO;
        ctx->quit_requested = true;
        pw_main_loop_quit(ctx->loop);
    } else if (state == PW_STREAM_STATE_UNCONNECTED && ctx->was_streaming) {
        ctx->quit_requested = true;
        pw_main_loop_quit(ctx->loop);
    }
}

static void on_stream_param_changed(
    void *data,
    uint32_t id,
    const struct spa_pod *param
) {
    struct capture_ctx *ctx = (struct capture_ctx *)data;
    if (param == NULL || id != SPA_PARAM_Format) {
        return;
    }

    if (spa_format_parse(param, &ctx->format.media_type, &ctx->format.media_subtype) < 0) {
        return;
    }
    if (ctx->format.media_type != SPA_MEDIA_TYPE_video ||
        ctx->format.media_subtype != SPA_MEDIA_SUBTYPE_raw) {
        return;
    }
    if (spa_format_video_raw_parse(param, &ctx->format.info.raw) < 0) {
        return;
    }

    ctx->has_format = true;

    uint8_t params_buffer[512];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(params_buffer, sizeof(params_buffer));
    const struct spa_pod *buf_params[1];

    uint32_t width = ctx->format.info.raw.size.width;
    uint32_t height = ctx->format.info.raw.size.height;
    uint32_t stride = width * 4;
    uint32_t frame_bytes = stride * height;

    buf_params[0] = (const struct spa_pod *)spa_pod_builder_add_object(
        &b,
        SPA_TYPE_OBJECT_ParamBuffers, SPA_PARAM_Buffers,
        SPA_PARAM_BUFFERS_buffers, SPA_POD_CHOICE_RANGE_Int(4, 2, 8),
        SPA_PARAM_BUFFERS_blocks, SPA_POD_Int(1),
        SPA_PARAM_BUFFERS_size, SPA_POD_CHOICE_RANGE_Int(frame_bytes, frame_bytes, frame_bytes * 2),
        SPA_PARAM_BUFFERS_stride, SPA_POD_CHOICE_RANGE_Int(stride, stride, stride * 2),
        SPA_PARAM_BUFFERS_dataType, SPA_POD_CHOICE_FLAGS_Int((1 << SPA_DATA_MemFd) | (1 << SPA_DATA_MemPtr))
    );

    pw_stream_update_params(ctx->stream, buf_params, 1);
}

static void on_stream_process(void *data) {
    struct capture_ctx *ctx = (struct capture_ctx *)data;
    struct pw_buffer *b = NULL;

    /* Drain to latest available buffer so we never lag behind Gamescope/KWin */
    while (true) {
        struct pw_buffer *next = pw_stream_dequeue_buffer(ctx->stream);
        if (!next) {
            break;
        }
        if (b) {
            pw_stream_queue_buffer(ctx->stream, b);
        }
        b = next;
    }

    if (!b) {
        return;
    }

    if (ctx->has_format && b->buffer->n_datas >= 1) {
        struct spa_data *d = &b->buffer->datas[0];
        uint32_t width = ctx->format.info.raw.size.width;
        uint32_t height = ctx->format.info.raw.size.height;
        uint32_t spa_fmt = ctx->format.info.raw.format;
        uint32_t stride = d->chunk->stride ? (uint32_t)d->chunk->stride : (width * 4);
        uint32_t offset = d->chunk->offset;
        uint32_t size = d->chunk->size;

        if (width > 0 && height > 0 && size > 0) {
            if (d->data != NULL) {
                const uint8_t *ptr = (const uint8_t *)d->data + offset;
                if (offset + size <= d->maxsize) {
                    ctx->on_frame(ctx->userdata, width, height, stride, spa_fmt, ptr, size);
                }
            } else if (d->type == SPA_DATA_MemFd && d->fd >= 0 && d->maxsize >= offset + size) {
                void *mapped = mmap(NULL, d->maxsize, PROT_READ, MAP_SHARED, d->fd, 0);
                if (mapped != MAP_FAILED) {
                    const uint8_t *ptr = (const uint8_t *)mapped + offset;
                    ctx->on_frame(ctx->userdata, width, height, stride, spa_fmt, ptr, size);
                    munmap(mapped, d->maxsize);
                }
            }
        }
    }

    pw_stream_queue_buffer(ctx->stream, b);
}

static const struct pw_stream_events stream_events = {
    PW_VERSION_STREAM_EVENTS,
    .state_changed = on_stream_state_changed,
    .param_changed = on_stream_param_changed,
    .process = on_stream_process,
};

static void on_poll_timer(void *data, uint64_t expirations) {
    struct capture_ctx *ctx = (struct capture_ctx *)data;
    if (ctx->stop_flag && *ctx->stop_flag != 0) {
        ctx->quit_requested = true;
        pw_main_loop_quit(ctx->loop);
    }
}

int steamos_pw_capture_run(
    int portal_fd,
    uint32_t target_node_id,
    const char *target_object,
    const volatile uint8_t *stop_flag,
    pw_frame_cb_t on_frame,
    void *userdata
) {
    pw_init(NULL, NULL);

    struct capture_ctx ctx;
    memset(&ctx, 0, sizeof(ctx));
    ctx.stop_flag = stop_flag;
    ctx.on_frame = on_frame;
    ctx.userdata = userdata;

    ctx.loop = pw_main_loop_new(NULL);
    if (!ctx.loop) {
        return -ENOMEM;
    }

    struct pw_loop * raw_loop = pw_main_loop_get_loop(ctx.loop);
    struct spa_source *timer = pw_loop_add_timer(raw_loop, on_poll_timer, &ctx);
    if (timer) {
        struct timespec val = { .tv_sec = 0, .tv_nsec = 100 * 1000 * 1000 };
        pw_loop_update_timer(raw_loop, timer, &val, &val, false);
    }

    ctx.context = pw_context_new(raw_loop, NULL, 0);
    if (!ctx.context) {
        pw_main_loop_destroy(ctx.loop);
        return -ENOMEM;
    }

    if (portal_fd >= 0) {
        int dup_fd = fcntl(portal_fd, F_DUPFD_CLOEXEC, 0);
        if (dup_fd < 0) {
            pw_context_destroy(ctx.context);
            pw_main_loop_destroy(ctx.loop);
            return -errno;
        }
        ctx.core = pw_context_connect_fd(ctx.context, dup_fd, NULL, 0);
    } else {
        ctx.core = pw_context_connect(ctx.context, NULL, 0);
    }

    if (!ctx.core) {
        pw_context_destroy(ctx.context);
        pw_main_loop_destroy(ctx.loop);
        return -ECONNREFUSED;
    }

    pw_core_add_listener(ctx.core, &ctx.core_listener, &core_events, &ctx);

    struct pw_properties *props = pw_properties_new(
        PW_KEY_MEDIA_TYPE, "Video",
        PW_KEY_MEDIA_CATEGORY, "Capture",
        PW_KEY_MEDIA_ROLE, "Screen",
        PW_KEY_APP_NAME, "steamos-crd-bridge",
        NULL
    );
    if (target_object && target_object[0] != '\0') {
        pw_properties_set(props, PW_KEY_TARGET_OBJECT, target_object);
    }

    ctx.stream = pw_stream_new(ctx.core, "steamos-crd-capture", props);
    if (!ctx.stream) {
        pw_core_disconnect(ctx.core);
        pw_context_destroy(ctx.context);
        pw_main_loop_destroy(ctx.loop);
        return -ENOMEM;
    }

    pw_stream_add_listener(ctx.stream, &ctx.stream_listener, &stream_events, &ctx);

    uint8_t buffer[1024];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buffer, sizeof(buffer));
    const struct spa_pod *params[1];

    params[0] = (const struct spa_pod *)spa_pod_builder_add_object(
        &b,
        SPA_TYPE_OBJECT_Format, SPA_PARAM_EnumFormat,
        SPA_FORMAT_mediaType, SPA_POD_Id(SPA_MEDIA_TYPE_video),
        SPA_FORMAT_mediaSubtype, SPA_POD_Id(SPA_MEDIA_SUBTYPE_raw),
        SPA_FORMAT_VIDEO_format, SPA_POD_CHOICE_ENUM_Id(
            5,
            SPA_VIDEO_FORMAT_BGRx,
            SPA_VIDEO_FORMAT_BGRx,
            SPA_VIDEO_FORMAT_BGRA,
            SPA_VIDEO_FORMAT_RGBx,
            SPA_VIDEO_FORMAT_RGBA
        ),
        SPA_FORMAT_VIDEO_size, SPA_POD_CHOICE_RANGE_Rectangle(
            &SPA_RECTANGLE(1920, 1080),
            &SPA_RECTANGLE(64, 64),
            &SPA_RECTANGLE(7680, 4320)
        ),
        SPA_FORMAT_VIDEO_framerate, SPA_POD_CHOICE_RANGE_Fraction(
            &SPA_FRACTION(60, 1),
            &SPA_FRACTION(0, 1),
            &SPA_FRACTION(120, 1)
        )
    );

    uint32_t connect_id = (target_node_id != 0) ? target_node_id : PW_ID_ANY;
    int res = pw_stream_connect(
        ctx.stream,
        PW_DIRECTION_INPUT,
        connect_id,
        (enum pw_stream_flags)(PW_STREAM_FLAG_AUTOCONNECT | PW_STREAM_FLAG_MAP_BUFFERS),
        params,
        1
    );

    if (res >= 0) {
        pw_main_loop_run(ctx.loop);
    } else {
        ctx.error_code = res;
    }

    pw_stream_destroy(ctx.stream);
    pw_core_disconnect(ctx.core);
    pw_context_destroy(ctx.context);
    pw_main_loop_destroy(ctx.loop);

    return ctx.error_code;
}
