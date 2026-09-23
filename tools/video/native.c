// Regression checks for the bundled GStreamer MSE implementation.
#include <gst/gst.h>
#include <gst/mse/mse.h>

static GMainLoop *loop;
static int phase;
static void complete(GstSourceBuffer *buffer, gpointer data) {
  GError *error = NULL;
  GArray *ranges = gst_source_buffer_get_buffered(buffer, &error);
  g_assert_no_error(error);
  g_assert_nonnull(ranges);
  g_assert_cmpuint(ranges->len, ==, 1);
  GstSourceBufferInterval range = g_array_index(ranges, GstSourceBufferInterval, 0);
  g_array_unref(ranges);
  g_assert_cmpuint(range.end, ==, 12 * GST_SECOND);
  if (phase++ == 0) {
    // updateend must wait until the demuxed samples have reached the coded-frame store.
    g_assert_cmpuint(range.start, ==, 0);
    g_assert_true(gst_source_buffer_remove(buffer, 0, 4 * GST_SECOND, &error));
    g_assert_no_error(error);
  } else {
    // Removal must delete samples, including the GOP ending exactly at the boundary.
    g_assert_cmpuint(range.start, ==, 4 * GST_SECOND);
    g_main_loop_quit(loop);
  }
}
int main(int argc, char **argv) {
  gst_init(&argc, &argv);
  g_assert_cmpint(argc, ==, 2);
  GstMseSrc *src = g_object_new(GST_TYPE_MSE_SRC, NULL);
  GstMediaSource *source = gst_media_source_new();
  gst_media_source_attach(source, src);
  GError *error = NULL;
  GstSourceBuffer *video = gst_media_source_add_source_buffer(source,
      "video/mp4; codecs=\"avc1.42c00b\"", &error);
  g_assert_no_error(error);
  // An empty second track must not leak the ready-state mutex and deadlock initialization.
  GstSourceBuffer *audio = gst_media_source_add_source_buffer(source,
      "audio/mp4; codecs=\"mp4a.40.2\"", &error);
  g_assert_no_error(error);
  loop = g_main_loop_new(NULL, FALSE);
  g_signal_connect(video, "on-update-end", G_CALLBACK(complete), NULL);
  gchar *bytes; gsize size;
  g_assert_true(g_file_get_contents(argv[1], &bytes, &size, &error));
  g_assert_no_error(error);
  g_assert_true(gst_source_buffer_append_buffer(video, gst_buffer_new_wrapped(bytes, size), &error));
  g_assert_no_error(error);
  g_main_loop_run(loop);
  g_assert_cmpint(phase, ==, 2);
  g_print("BRO_NATIVE_PASS: append completion, separate-track initialization, coded-frame removal\n");
  gst_media_source_detach(source);
  gst_object_unref(video);
  gst_object_unref(audio);
  gst_object_unref(source);
  gst_object_unref(src);
  g_main_loop_unref(loop);
  return 0;
}
