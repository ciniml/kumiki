# 06. グラフィック層のインターフェース設計（案）

クレート名はすべて仮称。構成は [09](09-crate-layout-license.md) を参照。

## 設計原則

1. **embedded-graphics の `DrawTarget` 互換を必ず保つ。** 既存のフォント、図形、UI（mousefood, kolibri, embedded-text など）がそのまま動くこと。
2. **速い経路は拡張トレイトで足す。** `DrawTarget` のイテレータ経由の経路では、連続したバッファであることが失われる（[03](03-ecosystem-graphics.md) §1）。そこで、矩形の塗りつぶし、生バッファの転送、変換しながらの転送、スプライトの転送を別のトレイトとして用意する。
3. **パネル（コマンドの手順）とバス（転送）を分離し、ピクセル変換をバスに渡す。** M5GFX の `pixelcopy_t` を渡す方式を Rust 流に置き換える（[01](01-m5gfx-internals.md) §1-2）。
4. **転送を重ねる仕組みはバスの内部に閉じ込め、外には安全な API だけを出す。** ゼロコピー DMA は「所有権を渡す」か「スコープ内で借用する」の 2 つの形で表す。
5. **HAL への依存はバスの実装クレートだけに置く。** core、panel、sprite は embedded-hal にすら依存しない（トレイトだけを持つ）。
6. **alloc は使わなくても動くようにする。** バッファは呼び出し側が `&'static mut` で渡せる。`alloc` feature を有効にすると Vec ベースのスプライトが使える。

## 層の構成

```
 アプリ / embedded-graphics / Slint / mousefood
        │  DrawTarget + FastDraw（拡張トレイト）
 ┌──────┴────────────────────────────────────────┐
 │ Surface 層                                    │
 │  Display<P>（パネルへ直接描画）               │
 │  Sprite<F, Buf>（RAM 上の描画面）             │
 │  FrameBuffer<F> + 更新領域の記録（部分更新）  │
 └──────┬────────────────────────────────────────┘
        │  Panel トレイト（ウィンドウ、回転、初期化、表示）
 ┌──────┴────────────────────────────────────────┐
 │ Panel 層                                      │
 │  DcsPanel<M: Model>（ILI9342C, GC9107, ST7789,│
 │   ST7735, GC9A01, ILI9881C(DSI), ST7123）     │
 │  FbPanel（DPI/RGB/DSI ビデオモード）          │
 │  EpdPanel（IT8951, PaperS3 パラレル EPD）     │
 └──────┬────────────────────────────────────────┘
        │  CommandBus / PixelBus トレイト（blocking / async）
 ┌──────┴────────────────────────────────────────┐
 │ Bus 層                                        │
 │  汎用: embedded-hal SpiDevice/SpiBus + DC     │
 │        embedded-hal-async SpiBus（RP, STM32） │
 │  ESP: SpiDmaBus（パイプライン）, I8080Bus,    │
 │       I2sParallelBus（ESP32）, DpiBus,        │
 │       DsiBus（esp-hal / レジスタ直叩き）      │
 └───────────────────────────────────────────────┘
```

## 1. ピクセル形式（`kumiki-gfx::pixel`）

```rust
/// 送信順（パネルが受け取るバイト列）で表したピクセル形式
pub trait PixelFormat: Copy + 'static {
    const BITS: u8;                     // 1,2,4,8,16,18(24),24
    type Raw: Copy + Default;           // 例: [u8; 2]（ビッグエンディアンの RGB565）
    type Color: embedded_graphics_core::pixelcolor::PixelColor;
    fn from_color(c: Self::Color) -> Self::Raw;
}

pub struct Rgb565Be;   // swap565 相当。SPI/i8080 パネルの標準
pub struct Rgb565Le;   // DPI/DSI のフレームバッファ（CPU の並び）
pub struct Rgb666Be;   // ILI9881C の 18bit など
pub struct Rgb888;
pub struct Gray4; pub struct Gray8;          // EPD
pub struct Indexed<const BPP: u8>;           // パレット付き 1/2/4/8bpp
```

- 変換はトレイト `Convert<Src, Dst>` で表し、組み合わせごとに単相化してインライン展開させる（M5GFX の `color_convert<TDst,TSrc>` に相当）。
- 実行時にならないと形式が決まらない場合（BSP が機種ごとに異なるパネルを返すときなど）に備え、関数ポインタ表 `CopyFn` も用意する。
- スプライトやフレームバッファは、**送信順の Raw の並び**で持つ。こうしておけば、送信時に変換が要らずゼロコピーで送れる（M5GFX の `swap565_t` と同じ考え方。[01](01-m5gfx-internals.md) §3）。
- embedded-graphics の `Rgb565` との相互変換を提供する。将来 `DataOrder` が入ったら（[03](03-ecosystem-graphics.md) §1）、それに合わせる。

### 画素の供給源（`PixelSource`）: `pixelcopy_t` に相当するもの

```rust
/// 出力 1 行分または 1 チャンク分のピクセルを、dst（送信順のバイト列）に書き込む。
/// 返り値は書いたピクセル数。透過色に当たったら途中で止まる。
pub trait PixelSource<Dst: PixelFormat> {
    fn fill(&mut self, dst: &mut [Dst::Raw]) -> usize;
    fn skip_transparent(&mut self, max: usize) -> usize { 0 }
    fn remaining(&self) -> usize;
}
```

用意する実装:

| 実装 | 用途 |
|---|---|
| `SliceSource<S>` | 変換付きのスライス転送 |
| `StridedSource` | 部分矩形の転送 |
| `PaletteSource<BPP>` | パレット付き画像 |
| `AffineSource` | 16.16 固定小数点による回転・拡縮 |
| `ColorIterSource<I>` | embedded-graphics の `fill_contiguous` に渡されるイテレータ |
| `TransparentSource` | 透過色付きの転送 |

## 2. バス層（`kumiki-gfx::bus`）

M5GFX の `IBus` を、Rust では次の 2 つのトレイトに分ける。

```rust
pub mod blocking {
    pub trait CommandBus {
        type Error;
        fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Self::Error>;
        fn read(&mut self, cmd: u8, dummy_bits: u8, buf: &mut [u8]) -> Result<(), Self::Error> {
            Err(/* Unsupported */)
        }
    }

    pub trait PixelBus: CommandBus {
        /// 送信順のバイト列をそのまま送る。返った時点で src を再利用してよい。
        /// （実装は DMA を使ってもよいが、返る前に完了している）
        fn write_bytes(&mut self, src: &[u8]) -> Result<(), Self::Error>;

        /// 同じパターンを count 回送る（塗りつぶし用）。FIFO の繰り返し送信や、
        /// 小さなパターンバッファの繰り返し DMA で実装する。
        fn write_repeat(&mut self, pattern: &[u8], count: u32) -> Result<(), Self::Error>;

        /// パイプライン転送。バスが持つ DMA バッファ（A/B）を順に渡すので、
        /// 呼び出し側は fill の中で変換して書き込む。
        /// fill が 0 を返すか total_bytes に達したら終わる。
        /// 実装は「B を fill している間に A を DMA で送る」ように重ねてよい。
        fn write_with<F>(&mut self, total_bytes: usize, fill: F) -> Result<(), Self::Error>
        where F: FnMut(&mut [u8]) -> usize;

        /// 転送中のものがあれば完了まで待つ
        fn flush(&mut self) -> Result<(), Self::Error>;
    }
}
pub mod asynch { /* 同じ形の async fn 版。fill は同期のクロージャのまま */ }
```

### ゼロコピー DMA の型

- `write_bytes(&[u8])` は**返る前に完了する**。借用の範囲内で完結するので安全である。
- 完了を待たずに戻る転送が必要なら、所有権を渡す API を使う。

```rust
pub trait DmaPixelBus: PixelBus {
    type Buffer: DmaBuffer;                       // esp-hal の DmaTxBuf などを包んだ型
    type InFlight: Transfer<Bus = Self, Buffer = Self::Buffer>;
    fn start_write(self, buf: Self::Buffer) -> Self::InFlight;    // バスとバッファの所有権を移す
}
pub trait Transfer { fn is_done(&self) -> bool; fn wait(self) -> (Self::Bus, Self::Buffer); }
```

  esp-hal の `SpiDma::write_buffer` と `I8080::send` は、まさにこの形の API になっている（[03](03-ecosystem-graphics.md) §6）。これをそのまま持ち上げる。
- async 版では `async fn write_bytes(&mut self, &[u8])` を await している間に他のタスクが動ける。同じタスク内で転送と次の処理を重ねたい場合は `write_with`（パイプライン）を使う。

### 汎用実装（可搬性）

| 実装 | 依存 | 重ね合わせ | 想定用途 |
|---|---|---|---|
| `SpiDeviceBus<SPI: SpiDevice, DC: OutputPin>` | embedded-hal 1.0 | なし（チャンクを順に送る） | どの MCU でも動く参照実装。RP2040/STM32 のブロッキング |
| `SpiBusBus<SPI: SpiBus, DC, CS>` | embedded-hal 1.0 | なし。CS を保持したまま続けて送れる | 専有バスで高速化する場合 |
| `AsyncSpiBus<SPI: embedded_hal_async::spi::SpiBus, DC>` | embedded-hal-async | タスク間で重なる。`write_with` は送信と変換を交互に行う | embassy-rp, embassy-stm32, esp-hal の Async |
| `EspSpiDmaBus` | esp-hal（`requires-unstable`） | **あり**（`write_buffer` と A/B 2 枚の DmaTxBuf） | ESP32/S3/C3/C6 |
| `EspI8080Bus` | esp-hal の `lcd_cam::lcd::i8080` | あり | S3 の 8/16bit パラレル |
| `EspI2sParallelBus` | esp-hal の `i2s::parallel` | あり | 無印 ESP32 のパラレル |
| `EspDpiBus` / `EspDsiBus` | esp-hal の `lcd_cam::lcd::dpi` / `mipi_dsi` | フレームバッファを走査（スキャンアウト） | S3 の RGB、P4（rev ≥ v3） |
| `P4v1DsiBus` | レジスタ直叩き（tab5-os 由来） | GDMA の LLI リング | P4 rev v1.x（Tab5 の初期ロット） |

`write_with` の ESP 実装は M5GFX の `writePixels` と同じ方式にする（[01](01-m5gfx-internals.md) §2.2）。

- 最初のチャンクは 64B 程度と小さくし、そこから倍々に大きくして最大 4KB〜16KB まで増やす。
- A を DMA で送っている間に B を fill し、`wait(A)` → `start(B)` の順で切り替える。

## 3. パネル層（`kumiki-gfx-panels`）

```rust
pub trait Panel {
    type Bus; type Format: PixelFormat; type Error;
    fn init(&mut self, delay: &mut impl DelayNs) -> Result<(), Self::Error>;
    fn size(&self) -> Size;                       // 回転を反映した大きさ
    fn set_rotation(&mut self, r: Rotation) -> Result<(), Self::Error>;
    fn set_invert(&mut self, on: bool) -> Result<(), Self::Error>;
    fn sleep(&mut self, on: bool) -> Result<(), Self::Error>;
}

/// 矩形ウィンドウにピクセルを流し込むパネル（MIPI-DCS、IT8951 など）
pub trait WindowPanel: Panel {
    /// 前回と同じ値なら CASET/RASET を省く（M5GFX の set_window_8 と同じ）
    fn set_window(&mut self, area: &Rectangle) -> Result<(), Self::Error>;
    fn bus(&mut self) -> &mut Self::Bus;           // ピクセルの送信はバスに直接任せる
}

/// メモリ上のフレームバッファをハードウェアが走査するパネル（DPI/RGB/DSI ビデオモード）
pub trait FramebufferPanel: Panel {
    fn back_buffer(&mut self) -> &mut [<Self::Format as PixelFormat>::Raw];
    fn flush_cache(&mut self, area: &Rectangle);   // キャッシュの書き戻し（P4 では必須）
    fn present(&mut self) -> Result<(), Self::Error>;   // ダブルバッファの入れ替え。vsync に同期
    fn accel(&mut self) -> Option<&mut dyn Accel2d> { None }  // PPA/DMA2D
}

/// 表示の更新を明示的に行うパネル（EPD）
pub trait RefreshPanel: Panel {
    fn refresh(&mut self, area: &Rectangle, mode: EpdMode) -> Result<(), Self::Error>;
    fn busy(&mut self) -> bool;
}
```

- `DcsPanel<B, M: Model>` は 1 つの汎用構造体で、機種ごとの差は `Model` トレイトの定数に集める。
  - 持たせる定数: 初期化コマンド表、メモリサイズ、表示サイズ、オフセット、MADCTL の表、COLMOD、invert の既定値、RDDID の期待値。
  - M5GFX の `Panel_LCD` と `Panel_ILI9342` などの分け方と同じ。
  - 初期化表は、mipidsi（MIT）と M5GFX（MIT / BSD-2）を参照して移植し、出典を明記する。
- リセットとバックライトは、パネルに `OutputPin` / `SetDutyCycle` として外から渡す。エキスパンダや PMIC 経由で制御する場合は BSP が実装を差し込む（stackchan-rs の `Aw9523Pin` と同じ方式。[02](02-local-assets.md)）。

## 4. Surface 層と拡張トレイト

```rust
/// 高速経路。DrawTarget を拡張する。
pub trait FastDraw: DrawTarget {
    fn fill_rect(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error>;
    fn blit<F: PixelFormat>(&mut self, area: &Rectangle, src: &[F::Raw], stride: usize)
        -> Result<(), Self::Error>;                                       // 必要なら変換しながら送る
    fn blit_source<S: PixelSource<Self::Format>>(&mut self, area: &Rectangle, src: S)
        -> Result<(), Self::Error>;                                       // パレット、アフィン、透過
    fn copy_rect(&mut self, src: Point, dst: &Rectangle) -> Result<(), Self::Error> { /* 読み戻しかスプライトで */ }
    fn batch<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R;          // startWrite/endWrite に相当
}
```

- `Display<P: WindowPanel>`
  - `DrawTarget` の実装では、各メソッドを次のように置き換える。

  | メソッド | 実装 |
  |---|---|
  | `fill_solid` | `fill_rect`（ウィンドウを設定してから `write_repeat`） |
  | `fill_contiguous` | `ColorIterSource` を `write_with` に流すパイプライン |
  | `draw_iter` | 横に連続するピクセルをまとめて送る（mipidsi の batch 相当） |
  | `clear` | 全画面の `fill_rect` |

  - 回転とオフセットは Panel 側で吸収する。クリッピングは Surface 側で一度だけ行う。
- `Sprite<F, B: AsMut<[F::Raw]>>`
  - RAM 上の描画面で、`DrawTarget` と `FastDraw` を実装する（塗りつぶしは行の複製、転送は `copy_from_slice`）。
  - `push_to(&mut impl FastDraw, pos, transparent: Option<F::Raw>)`、`push_affine(..., angle, zoom)`、`push_palette(..)` を持つ。
  - 内部 RAM の DMA 可能な領域に置けば、`blit` からゼロコピーで送れる。PSRAM 上のものも、S3/P4 では DMA が読めるのでゼロコピーで送れる。無印 ESP32 は PSRAM を DMA で読めないため、`write_with` 経由でコピーして送る。
- `FrameBuffer<F>` + `DirtyRegions<N>`
  - 書き換えた矩形を最大 N 個まで記録し、近い矩形は結合する。
  - `flush(&mut display)` で更新した部分だけを送る。
  - FramebufferPanel では、キャッシュの書き戻しと PPA による塗りつぶし・コピーに置き換える。
- ダブルバッファ（SPI パネル）
  - `DoubleBuffered<Display, FrameBuffer×2>` にする。A を DMA で送っている間に B へ描く。
  - S3 では PSRAM に 2 枚（150KB × 2）置ける。無印 ESP32 では内部 RAM が足りないので、行の帯（例: 320×40）単位で回す。

## 5. blocking と async の両対応

- **DrawTarget は同期 API なので、パネルへ直接描く Surface はブロッキングにする。**
- async の世界では「RAM 上の Surface（Sprite / FrameBuffer）に同期で描き、`flush().await` / `blit_async().await` で送る」形にする。lcd-async や stackchan-rs の `AsyncDisplay` と同じ方針である（[02](02-local-assets.md), [03](03-ecosystem-graphics.md) §4）。
- `Display<P>` には、`fill_rect_async` と `blit_async` を持つ `AsyncFastDraw` も実装する（バスが `asynch::PixelBus` の場合）。
- 2 つの版でロジック（クリッピング、ウィンドウの計算、ソースの生成）は共有し、バスの呼び出しだけを分ける。`maybe-async` 系のマクロは依存とデバッグのしやすさを考えて**使わない**。必要なら、小さな自前マクロで両方の版を生成する。
- **既定は async にするが、非同期ランタイムには依存しない**（決定事項）。
  - gfx の各クレートは `embedded-hal-async` のトレイトと、`core::future` だけを使う。executor、`embassy-time`、タスクの起動（spawn）には依存しない。
  - DMA の完了待ちは、HAL の割り込みと waker で実現する（esp-hal の async ドライバは executor から独立している）。
  - 待ち時間が必要な箇所（パネルの初期化など）は、外から渡された `DelayNs` を使う。
  - 詳細は [07](07-design-hwdesc.md) §7。

## 6. 可搬性（ESP32 以外）

- `kumiki-gfx` と `kumiki-gfx-panels` は、`embedded-graphics-core`、`embedded-hal`（`DelayNs` と `OutputPin` のトレイトだけ）、`embedded-hal-async` だけに依存する。
- RP2040/RP2350 では、`AsyncSpiBus` と embassy-rp の SPI DMA を組み合わせる。将来は PIO で i8080 バスを作る。
- STM32 では、`AsyncSpiBus` を embassy-stm32 と組み合わせる。FMC（i8080）や LTDC（RGB）は、`FramebufferPanel` を実装する形で後から追加できる。
- CI では thumbv6m（RP2040）、thumbv8m（RP2350）、riscv32imc（C3）でのビルド確認を必須にする。

## 7. EPD（Paper、PaperS3、CoreInk）

- `RefreshPanel` と、4bpp のグレー `FrameBuffer<Gray4>` を組み合わせる。
- ディザ（4×4 Bayer）は `PixelSource` のアダプタとして実装する。
- 更新モード（DU / GL16 / GC16 に相当）は `EpdMode` で指定する。
- PaperS3 のパラレル EPD は、LCD_CAM と波形 LUT のタスクで構成する。M5GFX の `Panel_EPD`（[01](01-m5gfx-internals.md) §7）や epdpi-esp を参考にし、後のマイルストーンで扱う。

## 8. UI フレームワークとの接続

- Slint: `LineBufferProvider` を実装するアダプタを作る。1 行描くごとに `blit` でパイプラインに流すか、行の帯をためてから DMA で送る。
- LVGL（oxivgl）: flush コールバックで `blit` を呼ぶアダプタを作る。優先度は低い。
