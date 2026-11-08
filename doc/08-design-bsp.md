# 08. BSP（M5Unified に相当するもの）の設計（案）

BSP は、[07](07-design-hwdesc.md) の HWD による記述から生成したコードと、機種をまたいで共通に使う API（M5Unified の `M5.Display`、`M5.Power`、`M5.BtnA` などに相当するもの）を組み合わせたものとして作る。

## 1. 機種は自動判別か、feature で切るか

**結論: チップは feature で指定し、そのチップ上の機種は実行時に自動判別する。feature で機種を固定することもできるようにする。**

| 方式 | 長所 | 短所 |
|---|---|---|
| 機種ごとの feature のみ | バイナリが最小で、型がすべて具体的になる | 同じ製品名でもロットで部品が違う（Core2 の PMIC、ILI9342C と E、Tab5 のパネル。[04](04-ecosystem-bsp.md) §10）ので、feature だけでは対応しきれない |
| 完全な自動判別（M5Unified 方式） | 1 つのバイナリで多くの機種に対応できる | Rust ではターゲットトリプルと esp-hal のチップ feature がビルド時に決まるので、チップをまたぐことはできない |
| **混合（推奨）** | チップの中では M5Unified と同じ使い勝手になり、ロットの違いにも実行時に対応できる | 機種ファミリー全体のドライバがリンクされる（feature で絞れる） |

- feature の構成
  - `chip-esp32`、`chip-esp32s3`、`chip-esp32p4`、`chip-esp32c6` … **必須で、どれか 1 つを選ぶ**。
  - `board-cores3`、`board-atoms3`、… **任意**。指定した機種だけを判別の候補にし、ドライバもその分だけリンクする。1 つだけ指定した場合は判別を省き、具体型の `Devices` を返す（追加コストなし）。
  - 何も指定しなければ、そのチップの全機種が判別の候補になる。
- ロットの違い（Core2 の PMIC など）は機種の違いではなく、[07](07-design-hwdesc.md) の `choices` で実行時に吸収する。そのため、`board-core2` を指定しても AXP192 と AXP2101 の両方に対応する。

## 2. API の形

### 2.1 Board トレイトではなく、分割して所有できる構造体を返す

```rust
let (id, m5) = m5_bsp::init(peripherals, m5_bsp::Config::default(), RESOURCES.take())?;
// id: BoardId（CoreS3 / AtomS3 / ...）
let m5_bsp::Devices { display, touch, power, imu, rtc, speaker, mic, buttons, led, ports, .. } = m5;
spawner.spawn(ui_task(display, touch))?;
spawner.spawn(sensor_task(imu.unwrap()))?;
```

- **トレイトのメソッドで各デバイスへの参照を返す形（`board.display()` のようなもの）は採らない。** embassy のタスクには `'static` で所有したものを渡す必要があり、ゲッター越しの参照ではそれを満たしにくいためである。
- 機種によって存在しないデバイスは `Option` で表す（AtomS3 にはタッチがない、StickC Plus2 には PMIC がない、など）。
- 各フィールドの型は、デバイスクラスのトレイト（`hwd-class`）を実装している。
  - 単一機種のビルドでは具体型になる。
  - ファミリーのビルドでは `AnyX` の enum になる。
- アプリが機種に依存しないように書けることを優先する。`Devices` のジェネリックな境界に `impl Display + ...` を使う書き方と、`AnyX` を直接受け取る書き方の両方ができるようにする。

### 2.2 デバイスクラス（`hwd-class`）の主な API

| クラス | 主なメソッド（blocking と async の両方。async 版は `_async` を付けるか、別トレイトにする） |
|---|---|
| `Display` | gfx の `FastDraw` / `DrawTarget` を実装する。ほかに `set_brightness(u8)`、`set_rotation`、`sleep` |
| `Touch` | `read() -> TouchPoints<N>`（座標はディスプレイの回転に合わせて変換済み）、`wait_for_touch().await` |
| `Power` | `battery_level() -> Option<u8>`、`is_charging()`、`vbus_present()`、`set_ext_output(bool)`（5V 出力）、`power_off()`、`deep_sleep(wake)`、`set_charge_current(mA)` |
| `Imu` | `accel()`、`gyro()`、`temperature()`。地磁気（BMM150 など）は `Magnetometer` として別に持つ |
| `Rtc` | `datetime()`、`set_datetime()`、`set_alarm()` |
| `AudioOut` | `play(&[i16], rate).await`、`set_volume`、`tone(freq, ms)`。I2S と DMA を使い、アンプとコーデックの制御を中に隠す |
| `AudioIn` | `record(&mut [i16]).await` |
| `Buttons` | `update()` または `events().await` で `ButtonEvent { id, kind: Pressed / Released / Click / LongPress / DoubleClick }` を得る |
| `Led` | `set(rgb)`（RMT / WS2812 など） |
| `Ports` | 外部コネクタ（Port A/B/C）のピンとバス。Unit をつなぐときに使う（[07](07-design-hwdesc.md) §8） |

### 2.3 ボタンのモデル

M5 のボタンには、次のような入力源がある。

- GPIO
- PMIC の電源キー（AXP192 / AXP2101 の PEK）
- IO エキスパンダのピン
- タッチパネル上の領域（Core2 の画面下の 3 つの丸）

M5Unified の `btn_rawstate_bits` と同じく、**入力源ごとのアダプタが生の状態ビットを集め、それに共通のデバウンス処理とクリック判定をかける**（[04](04-ecosystem-bsp.md) §7）。入力源の割り当ては HWD の `role = "button.a"` などで記述する。

## 3. ピンとバスの共有

- 内部 I2C（PMIC、タッチ、IMU、RTC、エキスパンダ、コーデックが同居する）
  - 既定は async の `I2cDeviceWithConfig` + `embassy_sync::Mutex<CriticalSectionRawMutex, _>`。デバイスごとにクロックを変えられる。
  - blocking のビルドでは `CriticalSectionDevice` を使う。
- 外部 I2C（Port A）は、内部とは別のコントローラにする。ピンを共有している機種（AtomS3 など）は、HWD でそのとおりに記述する。
- SPI
  - 表示が DMA で専有するのを基本にする。
  - SD カードと共有する機種では、表示の側にバスを一時的に手放す API（`with_bus_released`）を持たせる。
  - CoreS3 の GPIO35（MISO と D/C の共用）は `share_hooks` で切り替える（[01](01-m5gfx-internals.md) §6、[07](07-design-hwdesc.md) §7）。
- エキスパンダや PMIC 経由のピン（LCD のリセット、タッチのリセット、アンプのイネーブル）は、`OutputPin` として各ドライバに渡す（stackchan-rs の `Aw9523Pin` と同じ方式）。

## 4. 初期化の順序

HWD の依存グラフから生成する（[07](07-design-hwdesc.md) §6.2）。典型的な流れは次のとおり。

1. クロック、PSRAM、ヒープを用意し、**大きなバッファ（フレームバッファ、スプライト用の領域）を確保する**。Wi-Fi を起動する前に行う（[02](02-local-assets.md) の教訓 4）。
2. 電源保持の GPIO を立てる（StickC Plus2 の GPIO4 など）。
3. 内部 I2C を初期化し、機種を判別する（eFuse → I2C のプローブ → パネル ID）。
4. PMIC のレールを、LCD やタッチなどに必要なものから順に立ち上げる。決められた待ち時間も入れる。
5. IO エキスパンダを初期化し、リセット線を解除する。
6. 表示のバスとパネルを初期化し、画面を消去してから**バックライトを点ける**（初期化中のゴミが見えないようにするため）。
7. タッチを初期化する（GT911 は、INT 線の状態でアドレスを選ぶ起動シーケンスが必要）。
8. IMU と RTC を初期化する。
9. オーディオ（`lazy` のものは、アプリが要求したときに初期化する）。
10. ボタンと LED。

各段階は `#[inline(never)]` の関数として生成する。大きなオブジェクトは `StaticCell` に置く（スタックオーバーフロー対策。[02](02-local-assets.md) の教訓 3）。

## 5. 電源制御

- `Power` は、PMIC（AXP192 / AXP2101）、GPIO による電源保持（StickC Plus2）、IP5306（Basic / Fire）、PMS150G（PaperS3）、IP2326 + INA226（Tab5）の違いを吸収する。
- 提供する機能
  - 電池の電圧と残量、充電状態
  - 外部 5V 出力の ON/OFF（CoreS3 では AW9523 と AXP2101 の両方が関わる）
  - 電源オフ（PMIC への命令、または電源保持 GPIO を下げる）
  - ディープスリープと復帰要因の設定
  - レールの個別制御（上級者向け。`Rail` トレイト）
- 危険な操作を守る: レールの電圧の範囲、Tab5 の PI4IOE の電源ビットのように、触ると電源が落ちるものは、HWD に `protected` として記述し、生成されるコードで書き込めないようにする。

## 6. Config（M5Unified の `M5.config()` に相当するもの）

```rust
pub struct Config {
    pub board_hint: Option<BoardId>,     // 判別をしない、または判別に失敗したときの代わり
    pub cache_detect: bool,              // 判別結果を RTC メモリかフラッシュに保存する
    pub ext_power: ExtPower,             // 外部 5V 出力を有効にするか、自動にするか
    pub display: DisplayConfig,          // 回転、輝度の初期値、DMA バッファの大きさ
    pub audio: AudioConfig,              // サンプリングレート、使うかどうか
    pub i2c_int_hz: Option<u32>,
}
```

## 7. 最初の対象機種と、その違いの吸収

| 機種 | チップ | 表示 | 吸収すべき主な違い |
|---|---|---|---|
| CoreS3 / SE | esp32s3 | ILI9342C/E（SPI、GPIO35 を共用） | AXP2101 + AW9523、SE にはカメラがない |
| AtomS3 / AtomS3R | esp32s3 | GC9107 128×128（オフセット y=32） | PMIC がない。バックライトは PWM（GPIO16）。IMU は MPU6886 / BMI270 |
| Cardputer / ADV | esp32s3 | ST7789V2 240×135 | キーボード（74HC138 / TCA8418）、NS4168 / ES8311 |
| Core2 / v1.1 / v1.3 | esp32 | ILI9342C/E | AXP192 / AXP2101、MPU6886 / BMI270、タッチ領域のボタン |
| StickC Plus2 | esp32 | ST7789V2 135×240 | PMIC がない（GPIO4 で電源を保持） |
| Tab5 | esp32p4 | DSI ILI9881C / ST7123 / ST7121 | P4 の rev（v1 / v3）、パネルとタッチのロット違い |
| Paper / PaperS3 | esp32 / esp32s3 | IT8951 / パラレル EPD | EPD の更新モード |
