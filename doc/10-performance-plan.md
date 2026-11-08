# 10. 性能目標と検証方法

## 1. 理論上限（バスの帯域で決まる上限）

SPI のパネルは、どれだけ CPU 側を最適化しても「ピクセル数 × bpp ÷ SPI クロック」で決まる上限を超えられない。目標は上限に対する比率で決め、M5GFX と並べて比較する。

| 機種 | 解像度 | バス | 1 フレームのビット数 | 全画面の理論 fps |
|---|---|---|---|---|
| CoreS3 | 320×240, RGB565 | SPI 40MHz | 1,228,800 | **32.6 fps**（30.7 ms） |
| AtomS3 | 128×128, RGB565 | SPI 40MHz | 262,144 | 152.6 fps（6.55 ms） |
| StickC Plus2 / Cardputer | 240×135, RGB565 | SPI 40MHz | 518,400 | 77.2 fps（12.96 ms） |
| Dial | 240×240, RGB565 | SPI 80MHz | 921,600 | 86.8 fps |
| Tab5 | 720×1280, RGB565 | DSI 2 レーン 1040Mbps（DPI 80MHz） | – | 垂直同期で約 60 fps に制限（tab5-os の実測は 64 fps。[02](02-local-assets.md)） |

- SPI クロックの出典は [01](01-m5gfx-internals.md) §2.6。CASET/RASET/RAMWR のオーバーヘッドは、全画面の転送では無視できる。
- 40MHz を超えるクロック（例: ILI9342C を 80MHz）は動作保証の範囲外の実験として別枠で測り、既定値にはしない。

## 2. 性能目標

| 指標 | 最初のマイルストーン（M2）の目標 | 最終目標 |
|---|---|---|
| 全画面ゼロコピー転送（B2）の fps | M5GFX の **80% 以上** | M5GFX と同等（理論値の 95% 以上） |
| 変換しながらの全画面転送（B3） | M5GFX の 80% 以上 | 同等 |
| 全画面の塗りつぶし（B1） | M5GFX の 80% 以上 | 同等 |
| 小さな矩形の塗りつぶし（B7）の回数/秒 | M5GFX の 50% 以上 | 80% 以上（1 回あたりの固定費が効くため、まず形を作る） |
| スプライトの透過転送（B5） | 計測して記録する | M5GFX の 80% 以上 |
| 全画面転送中に CPU が空いている割合（B9） | 計測して記録する | async 版で 70% 以上 |
| embedded-graphics 標準の図形（B8） | mipidsi 0.10 の 2 倍以上 | – |

Tab5（M4）: present の fps は垂直同期に張り付くこと。PPA での全画面塗りつぶしは 6ms 以下（tab5-os の 5.1ms が参考）で、M5GFX の CPU 描画を上回ること。

## 3. ベンチマークの項目

両方の実装で、**同じシナリオ、同じ機種、同じクロック**で測る。

| ID | シナリオ | M5GFX 側 | Rust 側 |
|---|---|---|---|
| B1 | 全画面の単色塗りつぶしを N 回（色は毎回変える） | `fillScreen` | `FastDraw::fill_rect` / `clear` |
| B2 | 内部 RAM 上の RGB565（送信順）の全画面画像を転送 | `pushImageDMA`（swap565） | `blit::<Rgb565Be>` |
| B3 | RGB888 の画像を変換しながら全画面転送 | `pushImage(rgb888)` | `blit::<Rgb888>` |
| B4 | PSRAM 上の 320×240 16bpp スプライトを全画面転送 | `LGFX_Sprite::pushSprite`（PSRAM） | `Sprite::push_to` |
| B5 | 32×32 のスプライト 64 個を透過付きで毎フレーム転送 | `pushSprite(x,y,transp)` | `push_to(.., Some(transp))` |
| B6 | 回転・拡縮付きの転送（100×100 → 全画面） | `pushRotateZoom` | `push_affine` |
| B7 | ランダムな小さい矩形の塗りつぶし、drawPixel、drawLine、文字列描画の回数/秒 | 各 API | DrawTarget / FastDraw |
| B8 | embedded-graphics 標準の図形とテキスト | – | 本実装と mipidsi 0.10（`SpiInterface`）の比較 |
| B9 | 全画面の連続転送中に、別タスクが回せたカウンタの値 | FreeRTOS の別タスク | embassy の別タスク |

基準として測る対象（ベースライン）

- M5GFX: 最新のリリース（0.2.x 系）と arduino-esp32 3.x。版を固定し、コミットハッシュを記録する。
- mipidsi 0.10 + `SpiInterface`（`ExclusiveDevice<SpiDma>`）
- stackchan-rs の `Ili9342Dma`（[02](02-local-assets.md)）

## 4. 計測方法

- 時間の計測
  - Rust 側は `esp_hal::time::Instant`（または CPU のサイクルカウンタ）で測る。
  - M5GFX 側は `esp_timer_get_time()` で測る。
  - どちらも N=100 回の中央値と p95 を取る。
- 結果の出力: シリアルに CSV で出す（列は `bench,board,impl,variant,n,median_us,p95_us,fps,bytes_per_s`）。ホスト側のスクリプト（`bench/tools/`）で表にまとめ、`doc/bench-results/` に機種ごと・日付ごとに残す。
- バスの使用率: ロジックアナライザで SCK と CS を記録し、「実際にクロックが出ている時間 ÷ 経過時間」を求める。ソフトの計測値と突き合わせ、計測の誤りを検出する。
- 条件をそろえる
  - CPU 240MHz、PSRAM の設定、SPI クロック、バックライトの状態をそろえる。
  - Wi-Fi は止める。
  - Rust は opt-level=3 + LTO、Arduino は -O2（既定）で測り、ビルド条件も記録する。
- 回帰テスト: `bench` クレートを CI でビルドする。実機での計測は手動でよいが、結果ファイルの差分をレビューの対象にする。

## 5. 想定される律速要因と対策

| 要因 | 影響 | 対策 |
|---|---|---|
| esp-hal の `write_buffer` 1 回あたりの固定費 | 小さな転送が遅くなる | 64B 以下は FIFO（W レジスタ）へ直接書く経路を、esp-hal に提案するか自前で持つ |
| GDMA の 1 回の転送長の上限（S3） | 長い転送の継ぎ目で隙間が空く | チャンクを分けて連続して実行し、M5GFX と同じくループで再実行する |
| 無印 ESP32 は PSRAM を DMA で読めない | B4 が変換なしでも CPU コピーになる | パイプライン（`write_with`）で CPU のコピーと DMA を重ねる |
| CoreS3 で GPIO35 を MISO と D/C で共用している | パネルの読み出しや SD との共存で切り替えが必要 | バスのフックで出力の有効・無効を切り替える（[08](08-design-bsp.md)） |
| `DrawTarget::draw_iter` の 1 ピクセルごとの処理 | B8 が遅い | 横方向のランをまとめる。Sprite に描いてから blit する使い方を推奨する |
