# 08. ボード定義と BSP（M5Unified に相当するもの）の設計

BSP は、[07](07-design-hwdesc.md) の DTS から生成したコード（`kumiki-boards`）と、ボードをまたいで共通に使うデバイスクラスの API（`kumiki-class`。M5Unified の `M5.Display`、`M5.Power`、`M5.BtnA` などに相当するもの）を組み合わせたものとして作る。**特定のメーカーに依存しない汎用の仕組み**とし、M5Stack の製品は `boards/dts/m5stack/` に置くボード定義の 1 つの集まりとして扱う。

## 1. ボードは自動判別か、feature で切るか

**結論: チップは feature で指定し、そのチップ上のボードは実行時に自動判別する。feature でボードを固定することもできるようにする。**

| 方式 | 長所 | 短所 |
|---|---|---|
| ボードごとの feature のみ | バイナリが最小で、型がすべて具体的になる | 同じ製品名でもロットで部品が違う（Core2 の PMIC、ILI9342C と E、Tab5 のパネル。[04](04-ecosystem-bsp.md) §10）ので、feature だけでは対応しきれない |
| 完全な自動判別（M5Unified 方式） | 1 つのバイナリで多くのボードに対応できる | Rust ではターゲットトリプルと HAL のチップ feature がビルド時に決まるので、チップをまたぐことはできない |
| **混合（推奨）** | チップの中では M5Unified と同じ使い勝手になり、ロットの違いにも実行時に対応できる | 候補になるボードのドライバがすべてリンクされる（feature で絞れる） |

- feature の構成
  - `chip-esp32s3`、`chip-esp32p4v1`（将来 `chip-esp32p4`、`chip-esp32c6` など）… **必須で、どれか 1 つを選ぶ**。無印 ESP32 は対象外（決定事項）。
  - `board-m5stack-cores3`、`board-m5stack-atoms3`、… **任意**。
    - 指定したボードだけを判別の候補にし、ドライバもその分だけリンクする。
    - 1 つだけ指定した場合は判別を省き、具体型の `Devices` を返す（追加コストなし）。
  - 何も指定しなければ、そのチップのボードをすべて候補にする。
- 部品の差し替え（IMU、パネル、タッチ）は、DT の `status = "fail-needs-probe"`（[07](07-design-hwdesc.md) §4.1 (a)）で実行時に吸収する。参照関係まで変わる違い（Core2 v1.0 と v1.1）は、ボードの DTS を分けて判別する（同 (b)）。

## 2. API の形

### 2.1 Board トレイトではなく、分割して所有できる構造体を返す

```rust
let (id, dev) = kumiki_boards::init(peripherals, Config::default(), RESOURCES.take(), delay).await?;
// id: BoardId（M5StackCoreS3 / M5StackAtomS3 / ...）
let Devices { display, touch, power, imu, rtc, speaker, mic, buttons, led, ports, tasks, .. } = dev;
// tasks: バックグラウンドの処理（ボタンの監視、オーディオなど）。アプリが自分のランタイムで回す
```

- **トレイトのメソッドで各デバイスへの参照を返す形（`board.display()` のようなもの）は採らない。** 非同期のタスクに `'static` で所有したものを渡しやすくするためである。
- ボードによって存在しないデバイスは `Option` で表す（AtomS3 にはタッチがない、StickC Plus2 には PMIC がない、など）。
- 各フィールドの型は、デバイスクラスのトレイト（`kumiki-class`）を実装している。
  - 単一のボードのビルドでは具体型になる。
  - ファミリーのビルドでは `AnyX` の enum になる。

### 2.2 デバイスクラス（`kumiki-class`）の主な API

各クラスはブロッキング版と async 版の両方を持つ（async 版は別トレイトにする）。

| クラス | 主なメソッド |
|---|---|
| `Display` | kumiki-gfx の `FastDraw` / `DrawTarget` を実装する。ほかに `set_brightness(u8)`、`set_rotation`、`sleep` |
| `Touch` | `read() -> TouchPoints<N>`（座標はディスプレイの回転に合わせて変換済み）、`wait_for_touch().await` |
| `Power` | `battery_level() -> Option<u8>`、`is_charging()`、`vbus_present()`、`set_ext_output(bool)`、`power_off()`、`deep_sleep(wake)` |
| `Rail` | レギュレータの個別制御（DT の `*-supply` の参照先） |
| `Imu` / `Magnetometer` | `accel()`、`gyro()`、`temperature()` / `field()` |
| `Rtc` | `datetime()`、`set_datetime()`、`set_alarm()` |
| `AudioOut` / `AudioIn` | `play(&[i16], rate).await`、`set_volume`、`tone()` / `record(&mut [i16]).await` |
| `Buttons` | `events().await` で `ButtonEvent { id, kind: Pressed / Released / Click / LongPress / DoubleClick }` を得る |
| `Led` | `set(rgb)` |
| `Ports` | 外部コネクタ（Port A/B/C）のバスとピン（[07](07-design-hwdesc.md) §8） |

### 2.3 ボタンのモデル

ボタンの入力源は、GPIO、PMIC の電源キー（AXP の PEK）、IO エキスパンダのピン、タッチパネル上の領域（Core2 の画面下の 3 つの丸）とさまざまである。

- M5Unified の `btn_rawstate_bits` と同じく、**入力源ごとのアダプタが生の状態ビットを集め、それに共通のデバウンス処理とクリック判定をかける**（[04](04-ecosystem-bsp.md) §7）。
- DT では、標準の `gpio-keys` を拡張し、`gpios` には GPIO、エキスパンダ、PMIC の GPIO を指定できるようにする。PMIC のキーは専用の compatible（`x-powers,axp2101-pek` など）、タッチ領域は `kumiki,touch-keys` として書く。

## 3. ピンとバスの共有

- 内部 I2C（PMIC、タッチ、IMU、RTC、エキスパンダ、コーデックが同居する）
  - async の既定: `embassy-embedded-hal` の `I2cDeviceWithConfig` と `embassy-sync` の `Mutex`。どちらも executor に依存しないライブラリである（[07](07-design-hwdesc.md) §7）。デバイスごとにクロックを変えられる。
  - blocking のビルド: `CriticalSectionDevice`。
- 外部 I2C（Port A）は、内部とは別のコントローラにする。ピンを共有しているボード（AtomS3 など）は、DT にそのとおり書く。
- SPI
  - 表示が DMA で専有するのを基本にする。
  - SD カードと共有するボードでは、表示の側に、バスを一時的に手放す API（`with_bus_released`）を持たせる。
  - CoreS3 の GPIO35（MISO と D/C の共用）は、glue ノードで切り替える（[01](01-m5gfx-internals.md) §6、[07](07-design-hwdesc.md) §6.4）。
- エキスパンダや PMIC 経由のピン（LCD のリセット、タッチのリセット、アンプのイネーブル）は、DT の `*-gpios` で参照し、`OutputPin` として各ドライバに渡す（stackchan-rs の `Aw9523Pin` と同じ方式）。

## 4. 初期化の順序

DT の依存グラフから生成する（[07](07-design-hwdesc.md) §6.1）。典型的な流れは次のとおり。

1. クロック、PSRAM、ヒープを用意し、**大きなバッファ（フレームバッファ、スプライト用の領域）を確保する**。Wi-Fi を起動する前に行う（[02](02-local-assets.md) の教訓 4）。
2. 電源保持の GPIO を立てる（StickC Plus2 の GPIO4 など）。
3. 内部 I2C を初期化し、ボードを判別する（eFuse → I2C のプローブ → パネル ID）。
4. レギュレータ（`*-supply` の参照先）を、依存する順に立ち上げる。`regulator-*-delay-us` などの標準プロパティで待ち時間も入れる。
5. IO エキスパンダを初期化し、リセット線を解除する。
6. 表示を初期化し、画面を消去してから**バックライトを点ける**（初期化中のゴミが見えないようにするため）。
7. タッチを初期化する（GT911 は、INT 線の状態でアドレスを選ぶ起動シーケンスが必要）。
8. IMU と RTC を初期化する。
9. オーディオ。`kumiki,deferred-init` を付けたものは、アプリが要求したときに初期化する（Zephyr の `zephyr,deferred-init` に相当）。
10. ボタンと LED。

各段階は `#[inline(never)]` の関数として生成し、大きなオブジェクトは `StaticCell` に置く（[02](02-local-assets.md) の教訓 3）。

## 5. 電源制御

- `Power` は、PMIC（AXP192 / AXP2101）、GPIO による電源保持（StickC Plus2）、IP5306（Basic / Fire）、PMS150G（PaperS3）、IP2326 + INA226（Tab5）の違いを吸収する。
- 提供する機能: 電池の電圧と残量、充電状態、外部 5V 出力の ON/OFF、電源オフ、ディープスリープと復帰要因、レールの個別制御（`Rail`）。
- 危険な操作の保護: 触ると電源が落ちるライン（Tab5 の PI4IOE の bit4 など）は、DT の `kumiki,protected-lines` で宣言する。生成されたコードからは、そのラインを `OutputPin` として取り出せないようにする。

## 6. Config

```rust
pub struct Config {
    pub board_hint: Option<BoardId>,     // 判別をしない、または判別に失敗したときの代わり
    pub cache_detect: bool,              // 判別結果を RTC メモリかフラッシュに保存する
    pub ext_power: ExtPower,
    pub display: DisplayConfig,          // 回転、輝度の初期値、DMA バッファの大きさ
    pub audio: AudioConfig,
}
```

## 7. 最初にボード定義を書く対象

決定事項（2026-11-08）: 外部との連携は行わず、**利用者がよく使うボードの定義から書く**。順序は **CoreS3 / AtomS3 → Tab5**。**無印 ESP32 系（Core2、M5Paper、StickC Plus2 など）は、いまからサポートする意義が薄いので対象外**とする（設計上は除外しないが、ボード定義を書かない）。

| 優先 | ボード | チップ / HAL | 表示 | 吸収すべき主な違い |
|---|---|---|---|---|
| 1 | CoreS3 / SE | esp32s3 / esp-hal | ILI9342C/E（SPI、GPIO35 を共用） | AXP2101 + AW9523、SE にはカメラがない |
| 1 | AtomS3 / AtomS3R | esp32s3 / esp-hal | GC9107 128×128（y オフセット 32） | PMIC がない、PWM のバックライト、IMU が MPU6886 か BMI270 か |
| 2 | Tab5（**rev v1**） | esp32p4v1 / **kumiki-p4v1-hal**（tab5-os 由来） | DSI ILI9881C / ST7123 / ST7121 | esp-hal が使えない（[07](07-design-hwdesc.md) §9）、パネルとタッチのロット違い |
| 3 | Cardputer、PaperS3、Dial など ESP32-S3 系 | esp32s3 / esp-hal | ST7789V2 / パラレル EPD / GC9A01 | 必要になったら追加する |
| – | Core2、M5Paper、StickC Plus2 など無印 ESP32 系 | esp32 | – | **対象外** |
