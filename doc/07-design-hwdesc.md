# 07. ハードウェア記述層（HWD: Hardware Description）の設計（案）

## 0. 問題設定

no_std Rust では、HAL の抽象化（embedded-hal 1.0 / embedded-hal-async）と、チップの記述（esp-metadata、stm32-data）は整っている。しかし、**「このボードでは、どのバスがどのピンにあり、そこにどのデバイスがどのアドレスでつながり、リセット線や電源がどこから来るか」を表す標準的な層がない**。

PC には ACPI があり、Linux や Zephyr には Device Tree がある。no_std Rust では、BSP ごとに手書きのコードでこれを表しているのが現状である（[04](04-ecosystem-bsp.md) §2、Tock の components、atsamd の `bsp_pins!`）。

本章では、この層を **HWD** と呼び、ESP32 と M5Stack を最初の対象にしつつ、汎用の仕組みとして設計する。

```
 アプリ
   │  デバイスクラスのトレイト（Display / Touch / Imu / Power / ...）
 BSP（HWD から生成される）  ← 本章
   │  バインディング（compatible → Rust のドライバ型 + 生成方法）
 ドライバ（embedded-hal のトレイトに対してジェネリック）
   │  embedded-hal / embedded-hal-async
 HAL（esp-hal, embassy-rp, embassy-stm32, ...）
   │
 チップの記述（esp-metadata, stm32-data）  ← 既存。HWD はここを再発明しない
```

## 1. 先行事例と、そこから得た教訓

| 方式 | 例 | 長所 | 短所 |
|---|---|---|---|
| A. コードで記述する | Tock の components、atsamd/rp の `bsp_pins!`、assign-resources、esp-bsp（C） | 型安全で、追加コストがない。自由度が高い | 書き方が標準化されず、ツールで扱えない。ボードごとに似たコードが重複する |
| B. データからビルド時にコード生成する | **Zephyr DT**（とその Rust 対応）、**Hubris** の app.toml、**Ariel OS の sbd**（YAML）、**esp-board-manager**（YAML→C）、**M5GFX の board_spec**（JSON→C++） | 標準化でき、検証ツールを作れる。生成結果は追加コストがない | スキーマとジェネレータの保守が要る |
| C. 実行時にデータを解析する | Linux の FDT / ACPI | 1 つのバイナリで様々な構成に対応できる | 型が弱い（文字列で照合する）。ドライバはどのみち全部リンクされる。MCU では先例がほぼない |

出典

- Zephyr: <https://docs.zephyrproject.org/latest/build/dts/howtos.html>、<https://github.com/zephyrproject-rtos/zephyr-lang-rust>
- Hubris: <https://github.com/oxidecomputer/hubris/blob/master/build/i2c/src/lib.rs>
- Ariel OS sbd: <https://github.com/ariel-os/sbd>、<https://github.com/ariel-os/ariel-os/tree/main/boards>
- esp-board-manager: <https://github.com/espressif/esp-board-manager>
- M5GFX board_spec: <https://github.com/m5stack/M5GFX/tree/master/extras/board_spec>
- Linux の deferred probe: <https://docs.kernel.org/driver-api/driver-model/driver.html>
- U-Boot of-platdata（DT を C の構造体に変換）: <https://docs.u-boot.org/en/latest/develop/driver-model/of-plat.html>

### 教訓（設計に反映するもの）

1. **バスを親、デバイスを子とする木構造と、参照（phandle）でグラフを表す**（Zephyr DT）。リセット線や電源のように別のデバイスを指す参照は、初期化順序（依存関係の DAG）でもある。
2. **初期化順序を生成時に検証する。** Zephyr では、DT の依存関係と初期化の優先度の食い違いがよくある失敗の原因になっており、後から `check_init_priorities.py` が追加された。HWD では順序そのものを依存グラフから生成する。
3. **ドライバが見つからなければ生成を失敗させる**（Hubris）。型付きのアクセサを生成し、ペリフェラルは一度しか取り出せないようにする（所有権。zephyr-lang-rust の `get_instance()` と Ariel の `define_peripherals!`）。
4. **スキーマは厳格にする**（`deny_unknown_fields`、本物の enum）。Hubris は TOML で enum を扱いにくかったため、トポロジーを任意フィールドの集まりに平坦化して妥協しており、その結果、不正な記述を書きやすくなっている。
5. **I2C アドレスの表記を 1 つに決める**（7bit に統一する）。esp-board-manager の CoreS3 の記述では、7bit と 8bit の表記が混在している。
6. **リビジョンごとの部品の差し替えをデータで表し、実行時の判別順序も生成する**（M5GFX の board_spec）。M5 製品は同じ型番の中で PMIC やパネルが変わる（[04](04-ecosystem-bsp.md) §10）。ビルド時に選ぶだけの方式（Zephyr HWMv2 の `board@rev`）では足りない。
7. **チップの層とボードの層を分ける**（esp-metadata はチップだけを記述している）。
8. **値ごとに出典と検証済みかどうかを持たせる**（M5GFX の `verified`）。
9. **命令的な初期化を書ける抜け道が必要**（esp-board-manager の `setup_device.c`）。PMIC の手順のように、データでは書き切れないものがあるため。
10. **後付けの追加（シールドやオーバーレイ）を表せること**（Zephyr の shields、esp-board-manager の `board_amend.yaml`）。M5 の Unit や Module がこれに当たる。

## 2. 方式の結論

**方式 B（ビルド時のコード生成）を主とし、実行時の判別は「生成された候補の中から選ぶ」形に限定する。**
実行時にデータを解析する方式 C は、将来の拡張（§9）として、スキーマが両方に対応できるようにだけしておく。

- 生成されるのは普通の Rust コードである。バインディングの型が合わなければコンパイルエラーになる。単一機種のビルドでは追加コストがなく、alloc も使わない。
- 複数機種のビルド（機種ファミリー）では、違いのあるデバイスクラスだけを enum で切り替える（`AnyDisplay` など）。共通部分は 1 回だけ生成する。

## 3. データモデル

DT の考え方（ノード、`compatible`、`reg`、参照）を土台に、M5GFX の board_spec の考え方（choices / revisions / detect / provenance）を足す。

### 3.1 要素

| 要素 | 内容 |
|---|---|
| `chip` | SoC とパッケージ（例: `esp32s3`）。ピンの妥当性はチップの記述（esp-metadata）で検証する |
| `controller` | チップ内のペリフェラルのインスタンス（`i2c0`、`spi2`、`i2s0`、`lcd_cam`、`dsi`）。ピン、クロック、共有の方針、DMA チャネルを持つ |
| `device` | ボード上のデバイス。`compatible`、`on`（親のバス）、`reg`（I2C の 7bit アドレス、または SPI の CS）、プロパティ、参照、`role`（アプリから見た役割。例: `display.main`）を持つ |
| 参照 | `gpio = "&aw9523 p1_1 active-low"`、`supply = "&axp2101 aldo3"`、`irq = "&chip gpio21 falling"`、`backlight = "&axp2101 dldo1"` のように、他のデバイスの機能を指す |
| `rail` | 電源のレール（PMIC のチャネル、GPIO で制御する LDO、常時オン）。電圧、立ち上げ順、立ち上がり後の待ち時間を持つ |
| `connector` | 外部コネクタ（Port A/B/C、Grove、M-Bus）。オーバーレイ（Unit、Module）の接続先になる |
| `variant` | 機種ファミリーの中の 1 機種。共通部分を継承し、差分（`select` / `override`）と判別ルール（`detect`）を持つ |
| `provenance` | 値ごとの出典 URL と、検証済みかどうかの印 |

### 3.2 記述の例（構文は仮。CoreS3 を簡略化したもの）

```toml
[board]
id = "m5stack-cores3"
family = "m5stack-esp32s3"          # 同じチップで、まとめてビルドできるグループ
chip = "esp32s3"
provenance = "https://docs.m5stack.com/en/core/CoreS3"

[controller.i2c_int]
kind = "i2c"; instance = "I2C0"; sda = 12; scl = 11
frequency = 400_000
sharing = "auto"                     # 生成時に blocking / async の方針から選ぶ（§6）

[controller.spi_lcd]
kind = "spi"; instance = "SPI2"; sck = 36; mosi = 37; miso = 35
dma = "auto"
share_hooks = "m5_bsp::cores3::gpio35_miso_dc_mux"   # 抜け道（§7）

[device.pmic]
compatible = "x-powers,axp2101"; on = "i2c_int"; reg = 0x34
rails = { aldo3 = { mv = 3300 }, dldo1 = { role = "backlight" }, bldo1 = { mv = 3300 } }

[device.ioexp]
compatible = "awinic,aw9523b"; on = "i2c_int"; reg = 0x58
init = { p0_mode = "push-pull" }

[device.lcd]
compatible = "ilitek,ili9342c"; on = "spi_lcd"; cs = 3
role = "display.main"
dc = "&chip gpio35"                  # MISO と共用（share_hooks で切り替える）
reset = "&ioexp p1_1 active-low"
backlight = "&pmic dldo1"
supply = "&pmic aldo3"
props = { width = 320, height = 240, invert = true, rgb_order = "bgr", spi_hz = 40_000_000, read_hz = 16_000_000 }

[device.touch]
compatible = "focaltech,ft6336u"; on = "i2c_int"; reg = 0x38
role = "touch.main"; reset = "&ioexp p0_0 active-low"; irq = "&ioexp p1_2"
props = { follows = "lcd" }          # 座標系は lcd の回転に追従する

[device.imu]
compatible = "bosch,bmi270"; on = "i2c_int"; reg = 0x69; role = "imu.main"

[device.rtc]
compatible = "belling,bm8563"; on = "i2c_int"; reg = 0x51; role = "rtc.main"

[device.spk]
compatible = "awinic,aw88298"; on = "i2c_int"; reg = 0x36; role = "audio.out"
reset = "&ioexp p0_2 active-low"; i2s = "i2s0"
lazy = true                          # アプリが要求したときに初期化する（Zephyr の deferred-init に相当）

[detect]                             # ファミリー内でこの機種を識別するルール（AND）
steps = [
  { i2c_read = { bus = "i2c_int", addr = 0x34, reg = 0x03, eq = 0x4A } },
  { i2c_read = { bus = "i2c_int", addr = 0x58, reg = 0x10, eq = 0x23 } },
]

[[variant]]                          # CoreS3-SE: カメラと近接センサーがない
id = "m5stack-cores3-se"
remove = ["camera", "proximity"]
detect.steps = [ { i2c_nack = { bus = "i2c_int", addr = 0x21 } } ]   # GC0308（SCCB）が応答しない
```

Core2 の PMIC の違い（v1.0 は AXP192、v1.1 は AXP2101）のような差は、`choices` で表す。

```toml
[device.pmic]
on = "i2c_int"; reg = 0x34
choices = [
  { compatible = "x-powers,axp192",  when = { i2c_read = { addr = 0x34, reg = 0x03, eq = 0x03 } } },
  { compatible = "x-powers,axp2101", when = { i2c_read = { addr = 0x34, reg = 0x03, eq = 0x4A } } },
]
# lcd.reset なども choices ごとに付け替える（axp192: "&pmic gpio4"、axp2101: "&pmic aldo2"）
```

## 4. 構文（入力形式）について

意味（§3 のデータモデル）と構文は分けて考える。構文の候補は次の 3 つ。

| 候補 | 長所 | 短所 |
|---|---|---|
| **TOML（推奨）** | Rust の利用者にとって身近で、外部ツールが要らない。serde の `deny_unknown_fields` とタグ付き enum で厳格に検証できる | 深い入れ子が書きにくい。参照は文字列（`"&ioexp p1_1"`）で書くことになる |
| DTS（Device Tree Source）の一部 | 標準的な構文で、phandle が言語に組み込まれている。Zephyr のボード定義を流用できる | choices / variants / detect は独自に拡張するしかない。Zephyr の記述は C プリプロセッサのマクロに依存している。Rust でパーサを用意する必要がある |
| YAML | esp-board-manager や Ariel sbd と同じ形式で、取り込みが簡単 | 型の解釈に曖昧さがある（いわゆる Norway 問題）。インデントの間違いに弱い |

推奨は、**正の形式（source of truth）を TOML にし、取り込み用の変換器を用意する**こと。

- M5GFX の board_spec（JSON、MIT）から変換する。リビジョン、部品の差し替え、判別順序の情報をそのまま活かせる。
- esp-board-manager の YAML から変換する（約 35 の M5 機種をすばやく網羅できる。ライセンスは要確認）。
- Zephyr の DTS から変換する（`dtoolkit` などで DTB を読む）。

**利用者に決めてもらう事項**: 構文を TOML にするか DTS にするか（[11](11-roadmap.md) の未決事項 1）。

## 5. バインディング（compatible → ドライバ）

- バインディングは「`compatible` 文字列、Rust のドライバ型、生成関数、依存するもの（バス、ピン、電源）、プロパティのスキーマ、デバイスクラス」の組である。
- サードパーティのドライバクレートは HWD を知らないので、バインディングは**別のマニフェストと、つなぎのクレート**で持つ。zephyr-lang-rust の `dt-rust.yaml`（augment）と同じ考え方である。

```toml
# hwd-bindings/bindings/focaltech,ft6336u.toml
compatible = "focaltech,ft6336u"
class = ["touch"]
driver = { crate = "ft6336u-dd", version = "0.2" }
bus = "i2c"                                   # I2cDevice（共有されたもの）を受け取る
deps = { reset = "output-pin?", irq = "input-pin?" }
ctor = "hwd_bindings::ft6336u::new"           # fn new(i2c, reset, irq, &Props) -> Result<impl Touch, _>
props = { follows = { type = "ref", optional = true } }
```

- 生成器は、`compatible` に対応するバインディングがなければ**生成を失敗させる**（Hubris と同じ）。
- `ctor` は embedded-hal のトレイトでだけ型付けする。どの HAL の具体型を渡すかは、生成器が §6 の HAL アダプタから決める。
- デバイスクラスのトレイト（`hwd-class`）: `Display`（gfx の `FastDraw` などを返す）、`Touch`、`Imu`、`Magnetometer`、`Rtc`、`PowerManager`（電池、充電、外部 5V、電源オフ）、`Rail`、`Backlight`、`AudioOut`、`AudioIn`、`Buttons`、`Led`、`GpioExpander`、`Camera`、`Storage`。
- ドライバがクラスのトレイトを実装していない場合は、つなぎのクレートの中で newtype を作って実装する（孤児ルールへの対処。stackchan-rs の CoreS3 用の newtype と同じ。[02](02-local-assets.md)）。

## 6. HAL アダプタと生成されるコード

### 6.1 HAL アダプタ（汎用性の要）

生成器は特定の HAL のことを知らない。HAL ごとに「コントローラの作り方」を表すテンプレートを、小さなクレートで用意する。

- `hwd-hal-esp`: `i2c!(p.I2C0, sda=p.GPIO12, scl=p.GPIO11, hz=400_000)` → `esp_hal::i2c::master::I2c`、`spi_dma!(...)`、`i2s!(...)`、`pin_out!(...)`、`efuse_pkg()` など
- `hwd-hal-rp`（embassy-rp）、`hwd-hal-stm32`（embassy-stm32）は後から追加できる構造にする。
- ピンの妥当性（存在するか、機能が合っているか、重複していないか）は、チップの記述（esp-metadata-generated の `for_each_gpio!` など）を使って生成時に検証する。

### 6.2 生成されるコードの形（単一機種の場合）

```rust
// 生成されたコード（概略）
pub struct Devices {
    pub display: DisplayMain,          // 型エイリアス。具体型は Display<DcsPanel<EspSpiDmaBus, Ili9342c>>
    pub touch:   TouchMain,
    pub imu:     ImuMain,
    pub rtc:     RtcMain,
    pub power:   PowerMain,            // AXP2101 + レール
    pub audio_out: Lazy<AudioOutMain>, // lazy = true
    pub buttons: Buttons,
}
pub struct Resources { /* DMA バッファ、共有バスのセルなど。StaticCell で 'static にする */ }

pub fn init(p: esp_hal::peripherals::Peripherals, r: &'static mut Resources)
    -> Result<Devices, InitError>
{
    // 依存グラフをトポロジカルソートした順序で、まっすぐ並んだコードとして生成される
    let i2c_int = hwd_hal_esp::i2c!(p.I2C0, sda = p.GPIO12, scl = p.GPIO11, hz = 400_000);
    let i2c_int = r.i2c_int.init(Mutex::new(i2c_int));               // 共有の方針で決まる
    let pmic = hwd_bindings::axp2101::new(I2cDevice::new(i2c_int), &PMIC_PROPS)?;
    pmic.rail(Aldo3).enable(3300)?;  delay_ms(PMIC_RAIL_DELAY);
    let ioexp = hwd_bindings::aw9523b::new(I2cDevice::new(i2c_int), &IOEXP_PROPS)?;
    let lcd_reset = ioexp.pin(P1_1).into_output(ActiveLow);
    // ...lcd → touch → imu → rtc の順
    Ok(Devices { /* ... */ })
}
```

- 参照の解決や順序の決定は、すべて生成時に済ませる。循環参照や、宣言されていない参照は生成エラーにする。
- `Devices` の各フィールドは別々に所有できる。そのため、embassy のタスクにそれぞれを `'static` で渡せる。Board トレイトにゲッターを生やす形にしないのはこのためである（[08](08-design-bsp.md)）。

### 6.3 機種ファミリーの場合（1 つのバイナリで複数機種に対応する）

```rust
pub enum BoardId { CoreS3, CoreS3Se, AtomS3, AtomS3R, Cardputer, StickS3 /* ... */ }
pub enum AnyDisplay { Ili9342c(Display<..Ili9342c>), Gc9107(Display<..Gc9107>), St7789(Display<..St7789>) }
// AnyDisplay は DrawTarget / FastDraw / hwd_class::Display を match で委譲して実装する（生成）

pub fn init(p: Peripherals, r: &'static mut Resources) -> Result<(BoardId, Devices), InitError> {
    let id = detect(&mut p_shared)?;   // 生成された判別手順の決定木
    match id {                         // match の腕ごとに、ペリフェラルを別々の用途で move できる
        BoardId::CoreS3 => cores3::init(p, r),
        BoardId::AtomS3 => atoms3::init(p, r),
        // ...
    }
}
pub struct Devices { pub display: AnyDisplay, pub touch: Option<AnyTouch>, pub imu: Option<AnyImu>, /* ... */ }
```

- ピンの型は `AnyPin` / `degrade()` で揃える（単一機種の場合は具体型のままでよい）。
- 判別手順の生成
  1. eFuse のパッケージ種別（と P4 の rev）で候補を絞る。
  2. 電源保持の GPIO（StickC Plus2 の GPIO4 など）を最初に立てる。
  3. 安全なプローブ（I2C の ACK、ID レジスタ）を行う。
  4. パネル ID（RDDID）を読む。
  - 「他の機種で出力として使われているピンを駆動しない」順序を、生成時の制約解決で決める。M5GFX の `detector_order.json` に相当する。
  - 判別結果は RTC メモリかフラッシュにキャッシュしてよい（M5GFX の NVS と同じ。既定は無効）。
- **ビルドの単位**: ファミリーは同じチップの中に限る。ターゲットトリプルと esp-hal のチップ feature はビルド時に決まるので、ESP32 と S3 を 1 つのバイナリにすることはできない。

## 7. 共有バス、相互参照、抜け道

- **I2C の共有**: 生成時の方針で選ぶ。
  - async: `embassy-embedded-hal` の `I2cDeviceWithConfig` + `embassy_sync::Mutex`。デバイスごとの `frequency` をここに反映する。
  - blocking: `embedded-hal-bus` の `CriticalSectionDevice`（マルチコア向け）または `RefCellDevice`。
- **SPI の共有**
  - 表示が DMA で専有するのを基本にする。
  - SD カードなどと共有する場合は、`SpiDevice` を共有バス経由で渡す。
  - ピンの多重化（CoreS3 の GPIO35）は、`share_hooks` でトランザクションの前後に呼ぶ関数を差し込む。
  - SPI モードの異なるデバイスの共有には未解決の課題がある（embedded-hal #608）ので、HWD は `mode` をデバイスごとに持たせる。
- **ほかのデバイスの機能を指す参照**: エキスパンダや PMIC のピン、レールを `OutputPin`、`InputPin`、`SetDutyCycle`、`Rail` として取り出せるよう、バインディングに `provides`（例: aw9523b は 16 本の GPIO を提供する）を宣言する。生成器は、取り出す側と使う側の型を合わせる。
- **抜け道（命令的なフック）**
  - `hook = "path::fn"` で、指定した段階（電源投入の後、判別の後、初期化の後）にユーザーの Rust 関数を呼ぶ。
  - 引数は、その時点までに生成されたデバイスへの参照で、型付けされる。
  - 例: CoreS3 で 5V 昇圧を有効にするかどうかの判断や、Tab5 の PI4IOE の危険なビット（[02](02-local-assets.md)）を保護する処理。

## 8. オーバーレイ（Unit、Module、ユーザーによる拡張）

- オーバーレイは `connector` を参照して、デバイスを追加するファイルである（例: Port A の I2C に ENV IV Unit をつなぐ）。
  - Cargo の feature または環境変数（`HWD_OVERLAYS`）で指定する。
  - ユーザーのクレートの `build.rs` から指定することもできる。
- `optional = true` を付けたデバイスは、実行時にプローブして見つかった場合だけ `Option::Some` になる（Hubris の `removable` と `validate` に相当）。
- コネクタの種類（Grove I2C、UART など）とピンの対応は、M5GFX の `connector_types` を参考にする。

## 9. 将来: 実行時に解析するテーブル（ACPI のような方式）

- 同じデータモデルを、コンパクトなバイナリ表現（`dtoolkit` が扱える FDT、または独自の postcard 形式）にしてフラッシュのパーティションに置き、`hwd-runtime` が読み取って `Any*` を組み立てる。これで、自作の基板向けにファームウェアを作り直さずに済む。
- ただし、ドライバはすべてリンクされ、型の検証は実行時になる。そのため v1 では作らない。ここではスキーマが両方の方式に耐えることだけを確認しておく。

## 10. 汎用性の確認方法

- M5 以外に、少なくとも次の 2 つのボードを HWD で記述し、同じアプリが動くことを確かめる。
  - ESP32-S3-DevKitC + SPI の ST7789 モジュール
  - RP2040（Pico）+ ST7789。`hwd-hal-rp` を使い、embassy-rp で動かす。
- 生成器の単体テストでは、ゴールデンファイル（生成されたコードのスナップショット）を比較する。不正な記述（循環、ピンの重複、未知の compatible、アドレスの範囲外）では生成が失敗することも確かめる。
- ツールの CLI（`hwd check / resolve / gen / pintable`）を用意し、CI で全機種の記述を検証する。M5GFX の board_spec CLI と同じ考え方である。

## 11. 既存の仕組みとの関係

| 既存の仕組み | HWD との関係 |
|---|---|
| esp-metadata / stm32-data | チップの層として参照する（ピンの検証、ペリフェラル名） |
| M5GFX board_spec | 取り込み元（MIT）。リビジョンと判別の考え方を踏襲する |
| esp-board-manager | 取り込み元（約 35 機種）。ライセンスは要確認 |
| Zephyr DT | データモデルの土台。DTS からの取り込みは任意 |
| Ariel OS sbd | 目的が近い（「他プロジェクトでも使えることを期待」と明記している）。デバイスの記述が入ったら、相互に変換できるようにするか合流するかを検討する（[05](05-build-vs-contribute.md)） |
| device-driver クレート | ドライバの**レジスタ**の記述（HWD はボードの**トポロジー**の記述）。不足しているドライバを新しく書くときに使う |
