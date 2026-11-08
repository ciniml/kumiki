// M5GFX baseline for the kumiki benchmarks (doc/10-performance-plan.md).
// Same scenarios and CSV format as firmware/kumiki-bench:
//   bench,<board>,m5gfx,<id>,<frames>,<us_per_frame>,<fps>
//
// Build (ESP-IDF 5.4.x):
//   source ~/esp-idf/5.4.2/export.sh
//   idf.py set-target esp32s3 && idf.py build && idf.py -p /dev/ttyACMx flash monitor
#include <M5GFX.h>
#include <esp_heap_caps.h>
#include <esp_timer.h>
#include <stdio.h>

static M5GFX display;

static const char* board_name(void) {
  switch (display.getBoard()) {
    case m5gfx::board_t::board_M5StackCoreS3: return "cores3";
    case m5gfx::board_t::board_M5AtomS3: return "atoms3";
    default: return "unknown";
  }
}

static void report(const char* id, int frames, int64_t start) {
  int64_t us = esp_timer_get_time() - start;
  int per = (int)(us / frames);
  int fps_x100 = per > 0 ? 100000000 / per : 0;
  printf("bench,%s,m5gfx,%s,%d,%d,%d.%02d\n", board_name(), id, frames, per, fps_x100 / 100, fps_x100 % 100);
}

extern "C" void app_main(void) {
  display.init();
  display.setRotation(display.getBoard() == m5gfx::board_t::board_M5StackCoreS3 ? 1 : 0);
  display.setBrightness(200);
  const int W = display.width();
  const int H = display.height();
  const int frames = 60;
  printf("m5gfx-bench on %s (%dx%d)\n", board_name(), W, H);

  // B1: full-screen solid fill.
  const uint16_t colors[] = {TFT_RED, TFT_GREEN, TFT_BLUE, TFT_WHITE};
  int64_t t = esp_timer_get_time();
  for (int i = 0; i < frames; ++i) display.fillRect(0, 0, W, H, colors[i & 3]);
  display.waitDMA();
  report("B1_fill", frames, t);

  // B2: full-screen image in wire order (swap565), internal DMA-capable RAM, no conversion.
  auto* frame = (lgfx::swap565_t*)heap_caps_malloc(W * H * 2, MALLOC_CAP_DMA | MALLOC_CAP_INTERNAL);
  for (int y = 0; y < H; ++y)
    for (int x = 0; x < W; ++x) frame[y * W + x] = lgfx::swap565_t(y * 255 / H, 0, 255 - y * 255 / H);
  t = esp_timer_get_time();
  for (int i = 0; i < frames; ++i) display.pushImageDMA(0, 0, W, H, frame);
  display.waitDMA();
  report("B2_blit565", frames, t);

  // B3: RGB888 band (W x 40) converted to RGB565 on the fly.
  const int BAND_H = H >= 40 ? 40 : H;
  auto* band = (lgfx::bgr888_t*)heap_caps_malloc(W * BAND_H * 3, MALLOC_CAP_INTERNAL);
  for (int i = 0; i < W * BAND_H; ++i) {
    int x = i % W;
    band[i] = lgfx::bgr888_t(x * 255 / W, 128, 255 - x * 255 / W);
  }
  const int bands = (H + BAND_H - 1) / BAND_H;
  t = esp_timer_get_time();
  for (int i = 0; i < frames; ++i)
    for (int b = 0; b < bands; ++b) display.pushImage(0, b * BAND_H, W, BAND_H, band);
  display.waitDMA();
  report("B3_blit888", frames, t);

  // B5: 64 keyed 32x32 sprites per frame.
  LGFX_Sprite spr(&display);
  spr.setPsram(false);
  spr.setColorDepth(16);
  spr.createSprite(32, 32);
  spr.fillScreen(TFT_MAGENTA);
  spr.fillCircle(16, 16, 15, TFT_YELLOW);
  t = esp_timer_get_time();
  for (int f = 0; f < frames; ++f)
    for (int k = 0; k < 64; ++k) {
      int x = (k * 37 + f * 3) % (W - 16) - 8;
      int y = (k * 23 + f * 2) % (H - 16) - 8;
      spr.pushSprite(x, y, TFT_MAGENTA);
    }
  display.waitDMA();
  report("B5_sprite_keyed", frames, t);

  // B8: primitives and text straight to the display.
  display.setFont(&fonts::Font0);
  display.setTextColor(TFT_WHITE);
  t = esp_timer_get_time();
  for (int f = 0; f < frames; ++f) {
    for (int k = 0; k < 20; ++k) display.drawCircle((k * 13 + f) % W + 10, (k * 7) % H + 10, 10, TFT_CYAN);
    display.drawString("kumiki", 4, 4);
  }
  display.waitDMA();
  report("B8_eg_primitives", frames, t);

  printf("done\n");
  for (;;) vTaskDelay(1000);
}
