# 01. M5GFX / LovyanGFX が性能を出している仕組み

Rust で同等の性能を出すための「何を真似るべきか」の整理。

## 参照したソース

- 主: `~/repos/M5GFX`（ブランチ `fhd60`、commit ac74a0f、2023-10-29、`library.properties` は 0.1.10）。以下、接頭辞なしのパスはこのツリー基準。
- 新しい版: `~/repos/tab5_claude_client/components/M5GFX`（v0.2.20、2026-08-08）。ESP32-P4 / Tab5 / PaperS3 の確認だけに使い、`NEW:` と表記する。
- 上流: <https://github.com/m5stack/M5GFX>、<https://github.com/lovyan03/LovyanGFX>

> 注: 手元の主ツリーは古く、P4・DSI・RGB バス・PaperS3 のコードを含まない。実装に入る前に上流 master と突き合わせ直す必要がある。

## 1. レイヤ構造（パネルとバスの分離）

| 層 | 役割 | 出典 |
|---|---|---|
| `LGFXBase` / `LovyanGFX` / `LGFX_Device` | クリッピング、色変換（パネル形式の「raw color」を保持）、図形描画 | `src/lgfx/v1/LGFXBase.hpp:1307,1322`, `LGFXBase.cpp:196-211` |
| `IPanel` | ウィンドウキャッシュ、`startWrite`/`endWrite` のネスト管理、`writeFillRectPreclipped`、`writeImage(pixelcopy_t*, use_dma)`、`writePixels`、`setWindow`、`display()`（EPD 用） | `src/lgfx/v1/Panel.hpp:43-192` |
| `Panel_Device` | `config_t`（解像度、オフセット、回転、invert、`bus_shared` など）、初期化コマンド表のインタプリタ | `panel/Panel_Device.hpp:32-205`, `Panel_Device.cpp:149-176` |
| `Panel_LCD` | MIPI-DCS の共通処理（CASET/RASET/RAMWR、MADCTL、COLMOD） | `panel/Panel_LCD.cpp` |
| `Panel_ILI9342` など | 初期化表と寸法だけを持つ | `Panel_ILI9342.hpp:28-61` |
| `IBus` | `writeCommand`/`writeData`/`writeDataRepeat`/`writePixels(pixelcopy_t*)`/`writeBytes(use_dma)`、`wait`/`busy`、`getDMABuffer`（2 枚のバッファを交互に返す）、`addDMAQueue`/`execDMAQueue` | `src/lgfx/v1/Bus.hpp:43-117` |
| `ILight` / `ITouch` | バックライト / タッチ | `Light.hpp:28-33`, `Touch.hpp:39-83` |

設計上いちばん大事なのは次の点である。パネルはコマンドの手順とウィンドウを担当し、ピクセル形式の変換（`pixelcopy_t`）はバス側で実行する。パネルは変換器（`pixelcopy_t*`）をバスまで渡すので、バスは DMA バッファへ直接変換して書き込める。これで「変換」と「転送」を重ねられる。

## 2. ESP32 系での高速化

### 2.1 SPI（`platforms/esp32/Bus_SPI.cpp`。esp32/s2/s3/c3 共通）

- **IDF ドライバはバスとピンの確保にしか使わない。** `spi_bus_initialize` とダミーデバイスの追加だけを行う（`platforms/esp32/common.cpp:473-503`）。それ以降は、uncached アドレスに置いたレジスタポインタ（W0、CMD、USER、MOSI_DLEN、DMA_OUT_LINK）を直接叩く（`Bus_SPI.cpp:99-134`）。
- **D/C は GPIO W1TS/W1TC への直接書き込みで切り替える。** SPI_USR を立てる直前に行う（`Bus_SPI.cpp:276-279`）。
- **送信開始後に完了を待たない。** `writeCommand`/`writeData` は「前の転送が終わるのを待つ → レジスタを設定する → 実行する」の順で、実行後はすぐ戻る（`Bus_SPI.cpp:252-311`）。そのため次のコマンドの準備が転送と重なる。
- **塗りつぶしはメモリバッファも DMA も使わない。** 12 バイトのパターン（24bpp の整列も考慮）を W0-W15 に置く。SPI の FIFO はリングバッファとして振る舞うので、DLEN を大きく取って同じ 64 バイトを繰り返し送る（`Bus_SPI.cpp:313-414`、コメント `:377-379`）。
- **DMA 記述子は自前で組む。** `lldesc_t` を 4092 バイトごとに連結する（`Bus_SPI.cpp:1028-1050`）。GDMA 世代（S3）は 1 回の転送長に `SPI_MS_DATA_BITLEN` の上限があるため、ループで再実行する（`:591-603`）。ESP32 では長さが 4 の倍数のときだけバーストを有効にする。これは 80MHz で起きるデータ破損の回避策（`:564-569`）。
- **小さな転送は DMA を使わない。** 64 バイト以下は W レジスタへ memcpy する。1KB 未満で DMA を使わない指定のときは、フリップバッファへコピーしてから DMA で送る（`:520-544`）。
- **矩形の部分転送はスキャッタギャザーで送る。** 大きな画像の部分矩形を変換なしで送る場合は、行ごとに記述子を並べて 1 回の DMA で送る（`addDMAQueue`/`execDMAQueue`、`Bus_SPI.cpp:684-789`、`Panel_LCD.cpp:306-318`）。

### 2.2 変換と DMA のパイプライン（`Bus_SPI::writePixels`、`Bus_SPI.cpp:416-435`）

```
limit = (bytes==2) ? 32 : 24;
do { len = min(limit, length); if (limit <= 256) limit <<= 1;
     dmabuf = _flip_buffer.getBuffer(len*bytes);   // A/B を交互に取得
     param->fp_copy(dmabuf, 0, len, param);         // B に変換している間に A を DMA で送る
     writeBytes(dmabuf, len*bytes, true, true); }   // 前回分の完了だけを待って即開始
while (length -= len);
```

- チャンクは 32 → 64 → … → 512 px と倍々に大きくする。最初の DMA を早く始めるためである。
- `FlipBuffer`（`platforms/common.hpp:117-156`）は DMA 可能なバッファ 2 枚を交互に返す。

### 2.3 `Panel_LCD::writeImage` の分岐（`Panel_LCD.cpp:286-397`）

1. 変換が不要で、メモリ上も連続している場合は、元バッファから**ゼロコピーで DMA** 転送する。
2. 変換は不要だがストライドがある場合は、DMA キュー（スキャッタギャザー）を使う。
3. 変換が必要でバスが空いている場合は、`writePixels` のパイプラインで送る。
4. 変換が必要で、前の DMA がまだ転送中の場合は、行ごとに `getDMABuffer` へ変換して DMA で送る。
5. 透過色がある場合は、`fp_skip`/`fp_copy` で不透明な連続区間（ラン）に分け、ランごとに `setWindow` する。

### 2.4 パラレルバス（i8080）

- ESP32 は I2S を LCD モードで使う（`platforms/esp32/Bus_Parallel8.cpp:88-307`）。
- ESP32-S3 は LCD_CAM を使う。`esp_lcd_new_i80_bus` はリソースの確保にだけ使い、GDMA チャネルを探したあとはレジスタを直接叩く（`platforms/esp32s3/Bus_Parallel8.cpp:79-124`）。256 バイトのピンポンキャッシュ経由、または元バッファからのゼロコピーで送る（`:306-383`）。

### 2.5 ESP32-P4（`NEW:`）

- `Bus_DSI` は `Bus_ImagePush` を継承し、`esp_lcd_new_dsi_bus` と DBI を包んだもの（`NEW: src/lgfx/v1/platforms/esp32p4/Bus_DSI.cpp:29-83`）。
- `Panel_DSI` は RGB565、`num_fbs=1`、`use_dma2d=true` の DPI パネル。CPU が PSRAM 上のフレームバッファへ直接描き、`esp_cache_msync(C2M)` でキャッシュを書き戻す（`NEW: Panel_DSI.cpp:61-145`、`Panel_FrameBufferBase.cpp:27-67`）。
- **PPA は使っていない**（`ppa_`/`PPA` で grep しても該当なし）。つまり塗りつぶしやコピーを PPA/DMA2D でハードウェアに任せれば、M5GFX を上回る余地がある（実測例は [02](02-local-assets.md) の tab5-os を参照）。

### 2.6 機種ごとの SPI クロック（主ツリー、`src/M5GFX.cpp`）

| 機種 | パネル | 書き込み | 読み出し | 出典 |
|---|---|---|---|---|
| Core2 / Tough | ILI9342C | 40 MHz | 16 MHz | `M5GFX.cpp:866-867` |
| CoreS3 | ILI9342C | 40 MHz | 16 MHz | `M5GFX.cpp:1151-1152` |
| AtomS3 | GC9107（SPI3） | 40 MHz | 16 MHz | `M5GFX.cpp:1193-1195` |
| StickC Plus / Plus2 | ST7789 | 40 MHz | 15 MHz | `M5GFX.cpp:640,724` |
| Cardputer | ST7789V2 | 40 MHz | — | `M5GFX.cpp:1351` |
| Dial | GC9A01 | 80 MHz | — | `M5GFX.cpp:1236` |

機種判定のプローブは 8MHz で行う（`M5GFX.cpp:608`）。
理論上限は、40MHz SPI で 320×240×16bit を送る場合、約 1.23 Mbit/frame なので **約 32.5 fps** である（オーバーヘッドを除く）。CPU 側の処理がどれだけ速くても、この上限は超えられない。

## 3. ピクセル形式の変換

- `color_depth_t` の下位バイトはビット数を表す。上位ビットはパレットの有無と、バイトスワップしているかどうかのフラグ（`misc/enum.hpp:207-231`）。
- **メモリ上の標準形式はバイトスワップ済み（ワイヤ順）である。** RGB565 は `swap565_t`、RGB888 は `bgr888_t` として保持する（`colortype.hpp:261-290`）。スプライトの中身がすでに送信時の並びになっているので、転送時に変換が要らない。
- `pixelcopy_t`（`misc/pixelcopy.hpp:30-96`）
  - 元画像の座標を 16.16 固定小数点で持ち、出力 1 px ごとの増分も保持する。等倍コピーと回転・拡縮を同じ構造体で扱える。
  - `fp_copy` と `fp_skip` は関数ポインタで、テンプレートで単相化した実装が入る（`pixelcopy.cpp:27-76`）。対応する組み合わせは RGB 同士、パレット 1/2/4/8bpp からの変換、グレースケール、アンチエイリアス付きなど。
- ユーザーが指定した色は、`setColor` の時点で一度だけパネル形式へ変換する（`color_conv_t`、`colortype.hpp:771-811`）。

## 4. スプライト

- `Panel_Sprite` は RAM 上の「パネル」として実装されており、描画コードは LCD と完全に共通（`LGFX_Sprite.hpp:41,104`）。
- 割り当ては PSRAM か DMA 可能な内部 RAM のどちらか。PSRAM の確保に失敗した場合は内部 RAM で代用する（`misc/SpriteBuffer.cpp:116-143`）。
- `pushSprite`（`LGFX_Sprite.hpp:415-419`）
  - 内部 RAM 上にあり、深度が一致するスプライトは**ゼロコピーで DMA** 転送する。
  - PSRAM 上のスプライトは DMA を使わずに送る（小さい場合はフリップバッファにコピーしてから送る）。
- 回転・拡縮付きの転送（`pushRotateZoom`、`LGFXBase.cpp:1168-1376`）
  - 逆行列を 16.16 固定小数点で持つ。
  - 走査線ごとに有効区間を求め、その区間を `writeImage(..., h=1, use_dma=true)` で送る。
- スプライト同士の転送は、条件が揃えば memcpy で済ませる。塗りつぶしは 1 行を作ってから残りの行へ memcpy で複製する（`LGFX_Sprite.cpp:162-208,429-487`）。

## 5. 矩形転送の最適化

- **ウィンドウのキャッシュ**: パックした `xs|xe<<16` を前回値と比べ、同じなら CASET を省く（RASET も同様）。RAMWR は毎回送る（`Panel_LCD.cpp:450-470`）。回転すると無効化する。
- **塗りつぶし**: `setWindow` のあと `writeDataRepeat` を呼ぶ。2.1 の FIFO リング方式を使う（`Panel_LCD.cpp:250-259`）。
- **トランザクションのまとめ**: `startWrite`/`endWrite` はネストのカウンタになっており、最外側でだけバスのロックと CS の切り替えを行う（`Panel.hpp:96-97`）。
- **DMA の制約**: `use_dma=true` かつ変換なしの場合、呼び出し側のバッファがそのまま DMA で読まれる。`waitDMA()` が戻るまで、そのバッファを書き換えたり解放したりしてはいけない（`LGFXBase.hpp:400-412`）。**Rust ではこれを所有権または借用で表現できる**。ここが C++ に対して安全性で勝てる部分である。

## 6. 機種の自動判別（`src/M5GFX.cpp`）

- 前回の判別結果を NVS（キー "AUTODETECT"）にキャッシュする。判別は最大 4 回試す（`init_impl`、`:508-598`）。
- 基本の手段はパネル ID の読み出し（RDDID 0x04、`_read_panel_id`、`:471-486`）。SD カードが MISO を駆動しないよう、事前に SD カードを SPI モードにしておく（`:452-469`）。
- ESP32 の場合は、efuse のパッケージ種別で最初に枝分かれする（`:620`）。
  - PICO-D4: StickC / StickCPlus / CoreInk
  - V3-02: StickCPlus2
  - D0WDQ6: I2C の 0x34 にある AXP の ID レジスタ 0x03 を読み、0x03 なら AXP192、0x4A なら AXP2101 と判定する。そこから Core2 v1.0 / v1.1 / Tough / Station を区別する（`:758-934`）。
  - Basic Core は、GPIO33 の状態から invert の有無を判別する（`:946-980`）。
  - M5Paper は IT8951 の `GET_DEV_INFO` で判別する（`:983-1084`）。
- ESP32-S3 の場合
  - CoreS3: AXP2101 の 0x03 が 0x4A、かつ AW9523B（0x58）の 0x10 が 0x23 であることで判定する（`:1097-1174`）。
    - **GPIO35 は MISO と LCD の D/C で共用**されており、出力の有効・無効を切り替えて使い分けている（`:338-352`）。
    - LCD のリセットは AW9523 経由、バックライトは AXP2101 の DLDO1。
  - AtomS3（GC9107 の ID が 0x079100、`:1176-1217`）、Cardputer（ST7789V2、`:1333-1377`）、Dial、DinMeter。
- `NEW:` 新しい版にある判別処理
  - PaperS3: GT911 のプローブで判別し、EPD を i80 で駆動する（`NEW: M5GFX.cpp:1575-1640`）。
  - Tab5: PI4IOE 経由でリセットし、DSI 越しの読み出しで ST7123 と ILI9881C を区別する（`NEW: M5GFX.cpp:2180-2290`）。

## 7. EPD

- `Panel_IT8951`（M5Paper）
  - 4bit グレーで、4×4 Bayer ディザを使う。
  - 更新モードは fastest→DU4、fast→DU、text→GL16、quality→GC16 に対応させている。
  - 変更があった矩形をためておき、`display()` でまとめて更新する（`panel/Panel_IT8951.cpp:41,448-660`）。
- `Panel_GDEW0154M09`（CoreInk）: 1bpp のフレームバッファと Bayer ディザ。
- `NEW: Panel_EPD`（PaperS3）
  - 波形 LUT をソフトウェアで駆動する。
  - 4bpp のフレームバッファ、ピクセルごとの状態バッファ、DMA 用の行バッファを持ち、専用タスクで回す（`NEW: platforms/esp32/Panel_EPD.cpp:82-356,887`）。

## 8. ライセンス

- M5GFX: MIT（`~/repos/M5GFX/LICENSE`、Copyright (c) 2021 M5Stack）
- 同梱の lgfx/v1 コアは LovyanGFX 由来で、FreeBSD（BSD 2-Clause）ライセンス（`src/lgfx/v1/LGFXBase.hpp:1-16`）。LovyanGFX の `license.txt` には、Adafruit_ILI9341（MIT）、Adafruit_GFX（BSD）、TFT_eSPI（FreeBSD）の表記も含まれる。
- 同梱フォント: efont（BSD-3）、IPA フォント（IPA Font License）、GFXFF。
- **Rust への移植（派生物）は可能。** ただし著作権表示とライセンス文の保持が必要になる。初期化コマンド表や判別手順を参照・移植する場合は、`THIRD_PARTY_NOTICES` に M5GFX の MIT 表記と LovyanGFX の BSD-2 表記を載せる方針とする。詳しくは [09](09-crate-layout-license.md)。

## Rust 実装に持ち込むべき要点

1. パネル（コマンドの手順）とバス（転送）を分離し、変換器をバス側に渡す。
2. 送信開始後に待たない API にする。「次の転送の直前に、前の転送の完了だけを待つ」方式。
3. 変換と DMA を、フリップバッファと倍々に大きくするチャンクで重ねる。
4. 塗りつぶしは FIFO の繰り返し送信（ESP32 固有の最適化）か、小さなパターンバッファを繰り返し DMA で送る方式にする。
5. ウィンドウ（CASET/RASET）をキャッシュする。
6. スプライトはワイヤ順のバイト列で保持し、内部 RAM にあればゼロコピーで DMA 転送する。
7. ゼロコピー DMA のライフタイムは、借用で静的に保証する。
