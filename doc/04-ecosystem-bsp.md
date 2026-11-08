# 04. BSP とデバイスドライバのエコシステム（2026-11-08 時点）

バージョン、日付、ライセンス、依存関係は crates.io の API（`https://crates.io/api/v1/crates/<name>`）で、リポジトリの活動状況は GitHub の API で確認した。表の「eh1」は embedded-hal 1.0 に依存していること、「async」は embedded-hal-async に依存していることを表す。
各クレートの URL は `https://crates.io/crates/<name>` と `https://docs.rs/<name>` である。表にはリポジトリの URL を載せた。

## 1. esp-rs と BSP

- **esp-rs 公式の BSP クレートは存在しない。** `esp32c3-devkit`、`esp-wrover-kit` などは crates.io にも GitHub にも見当たらない。
- esp-hal の examples は、チップと機能ごとに分かれている。ボードごとには分かれていない。<https://github.com/esp-rs/esp-hal/tree/main/examples>
- esp-generate に BSP テンプレートを追加する提案（PR #311）は閉じられた。それ以前の issue #167 も not planned で閉じられている。**esp-rs は BSP を扱わない方針と読める**（明文化された方針は見つからなかったので、推測である）。
  - <https://github.com/esp-rs/esp-generate/pull/311>
  - <https://github.com/esp-rs/esp-generate/issues/167>
- `esp-bsp` 0.4.1（2024-12）は個人のリポジトリで、休眠している。m5stack-core2 / cores3 / fire 向けの feature を持つが、古い esp-hal 向けである。<https://github.com/georgik/esp-bsp-rs>
- esp-hal 1.2.2 の安定版 API は gpio, i2c, spi, uart, efuse, rng, time, clock。i2s, dma, ledc, rmt, timer, delay などは unstable 扱い。<https://docs.rs/esp-hal/1.2.2/esp_hal/index.html>
- **ESP32-P4 は rev v3.0 以降のみ対応する。**
  - README: <https://github.com/esp-rs/esp-hal/blob/main/esp-hal/README.md>
  - rev v3.0 より前の silicon 向けには、コミュニティのフォーク `esp-hal-p4-pre-v3` がある（rev 1.3 が対象）: <https://github.com/naoNao89/esp32p4-pre-v3-kit>
  - M5Unified は `revision >= 300` かどうかで StampP4 と StampP4X を分けており、Tab5 にも `board_M5Tab5` と `board_M5Tab5X` がある。v3 silicon を積んだ「X」版が存在すると読める（`src/M5Unified.inl`）。

## 2. 既存の M5Stack 向け Rust クレート

| クレート | 版 / 日付 | ライセンス | 環境 | 備考 |
|---|---|---|---|---|
| `m5stack-core` | 0.6.1 / 2026-08 | MIT/Apache | no_std, esp-hal =1.1.2, embassy | Fire27 と CoreS3 向け。lcd-async と async-button を使う。<https://github.com/emobotics-dev/m5stack-core> |
| `core-s3` | 0.5.6 / 2026-10 | MIT | no_std, esp-hal =1.2.2 | CoreS3 専用。aw9523b, power, touch, motion, rtc, audio, sd のモジュールを持つ。<https://github.com/anapeksha/core-s3-rs> |
| `cardputer-adv` | 0.4.4 / 2026-10 | MIT/Apache | no_std, esp-hal ~1.2.2, mipidsi | <https://github.com/tszarzynski/cardputer-adv> |
| `oxstack-*` | 0.2.1 / 2026-10 | MIT（drivers は MIT AND BSD-3） | no_std, esp-hal =1.2.2, async | bmi270, es8311, st7789, tca8418 のドライバを持つ。<https://github.com/theashraf/oxstack> |
| `m5dial-bsp` | 0.7.3 / 2026-05 | 標準外のライセンス | no_std | <https://codeberg.org/antoinezen/m5dial-bsp> |
| `m5unified` | 0.3.8 / 2026-05 | MIT/Apache | **std**（M5Unified の C++ を FFI で呼ぶ） | <https://github.com/mfiumara/m5unified-rs> |

- crates.io で見つからなかったもの: core2、atoms3、m5stickc、m5paper、papers3、tab5。
- GitHub にだけあるもの
  - Core2 v1.1 のいちばん良い参考実装: <https://github.com/okhsunrog/m5core2v1-1_demo_rust>（esp-hal + embassy、AXP2101、FT6336U、BM8563、NS4168）
  - CoreS3: <https://github.com/andymai/stackchan-kai>
- **問題点**
  - 多くのクレートが esp-hal の版を `=1.1.2` や `=1.2.2` のように厳密に固定しているので、組み合わせて使えない。
  - どれも 1 機種専用で、複数機種を共通に扱う仕組みがない。

## 3. 電源 IC と IO エキスパンダ

| デバイス | 推奨クレート | 版 / 日付 | eh1 | async | 備考 |
|---|---|---|---|---|---|
| AXP192 | `axp192-dd` | 0.4.0 / 2026-09 | ✓ | ✓ | device-driver ベース。<https://github.com/okhsunrog/axp192-dd>。ほかに `axp192` 0.2.0（休眠） |
| AXP2101 | `axp2101-dd` | 0.3.1 / 2026-09 | ✓ | ✓ | <https://github.com/okhsunrog/axp2101-dd>。ほかに `axp2101-embedded` 0.3.0 |
| IP5306 | **なし** | – | – | – | Basic / Fire に必要 |
| AW9523B | `embedded-aw9523` | 0.3.0 / 2026-06 | ✓ | ✓ | <https://github.com/DanNixon/eio-aw9523> |
| PI4IOE5V6408（Tab5） | **なし** | – | – | – | `port-expander` が対応しているかは未確認 |

okhsunrog 氏の `*-dd` 系（axp192、axp2101、ft6336u、pcf8563）は、同じ設計でブロッキング版と async 版を 1 つのソースから提供しており、2026-09 に揃って更新されている。**第一候補にする。**

## 4. タッチ

| デバイス | 推奨クレート | 版 / 日付 | eh1 / async | 備考 |
|---|---|---|---|---|
| FT6336U | `ft6336u-dd` | 0.2.0 / 2026-09 | ✓ / ✓ | <https://github.com/okhsunrog/ft6336u-dd>。ほかに `ft6x36` 0.6.2 |
| GT911 | `gt911` | 0.3.0 / 2025-02 | ✓ / ✓ | 29 万ダウンロード。<https://github.com/ninjasource/gt911> |
| CST816S | `cst816s-async` | 0.2.0 / 2026-10 | ✓ / ✓ | <https://github.com/Sir-Thomas/cst816s-async> |
| FT5x06、CST820、ST7123 のタッチ | **なし** | | | |

## 5. IMU、RTC、センサー

- **MPU6886**: eh1 に対応したクレートは公開されていない（`mpu6886` 0.1.0 は eh0.2 のまま）。**新しく書く必要がある。**
- **BMI270**: `bmi2` 0.1.2（2026-03、eh1、ブロッキング）。<https://github.com/qrasmont/bmi2>
- **BMM150、SH200Q、LTR-553、RX8130CE**: クレートがない。
- **BM8563 / PCF8563**: `pcf8563-dd` 0.4.0（2026-09、eh1 + async、BM8563 対応を明記）。<https://github.com/okhsunrog/pcf8563-dd>

## 6. オーディオ

- `es8311` 0.1.1、`es7210` 0.1.0（いずれも 2026 年に出たばかりで、まだ若い）
  - <https://github.com/QuackHack-McBlindy/es8311>
  - <https://github.com/QuackHack-McBlindy/es7210>
- **AW88298（CoreS3）と ES8388（Tab5）はクレートがない。** M5Unified のレジスタ設定手順（`src/M5Unified.inl` の 540〜600 行付近と 856〜870 行付近）を移植の参考にする。
- NS4168（Core2、Cardputer）は制御インターフェースを持たない。I2S で音を送り、イネーブルピンを制御するだけ（Core2 では AXP192 の IO2）。
- esp-hal 1.2.2 の I2S（unstable）
  - TDM / Philips / PCM、MCLK、PDM に対応する。`into_async()` もある。
  - 循環転送には `DmaTxStreamBuf` を使う。
  - 全二重の issue（#2939）は解決済み。
  - <https://docs.rs/esp-hal/1.2.2/esp_hal/i2s/master/index.html>
  - <https://github.com/esp-rs/esp-hal/issues/2939>
  - 注意: 循環 DMA は `Late` エラーから復帰できない（[02](02-local-assets.md) の stackchan-rs の教訓）。

## 7. ボタン

- `button-driver` 0.2.5（embassy と eh1 に対応）。<https://github.com/maxwase/button-driver>
- `async-button` 0.2.0 は embassy-time 0.4 に固定されており、esp-hal 1.2 が使う 0.5 系と合わない。
- `debouncr` 0.2.2 は依存のない純粋なロジックで、ビットマスクを周期的に読む方式（ポーリング）に向く。
- **M5 のボタンの多くは GPIO ではない**（AXP の電源キー、AW9523 や PI4IOE のピン、タッチ領域）。M5Unified の `btn_rawstate_bits` と同じく、生の状態を集めたビットマスクに対して共通のデバウンス処理をかける方式が必要になる（`src/M5Unified.inl` 2969〜3170 行付近）。

## 8. 電子ペーパー

- M5Paper（SPI 接続の IT8951E）: `it8951` 0.5.1（eh1、ブロッキングのみ、alloc 必須、M5Paper での動作は未確認）。<https://github.com/pbert519/it8951>
- PaperS3（パラレル接続の ET047TC1）
  - `epdpi`/`epdpi-esp` 0.1.0 と `lilygo-epd47` は、**どちらも GPL-3.0**。
    - <https://github.com/melastmohican/epdpi>
    - <https://github.com/fridolin-koch/lilygo-epd47-rs>
  - 寛容なライセンスで出すなら、自前で書き直す（クリーンルーム）か、GPL のコードを任意の feature に隔離する必要がある。

## 9. バスの共有

- `embedded-hal-bus` 0.3.0（2025-01）: I2C 用と SPI 用に `RefCellDevice`、`CriticalSectionDevice`、`AtomicDevice` などがある。<https://docs.rs/embedded-hal-bus/0.3.0>
- `embassy-embedded-hal` 0.6.0（2026-03）
  - async 版の `shared_bus::asynch::i2c::{I2cDevice, I2cDeviceWithConfig}`。`embassy_sync::Mutex` で 1 本のバスを共有する。<https://docs.rs/embassy-embedded-hal/0.6.0>
  - esp-hal 1.2.2 は embassy-embedded-hal ^0.6 と embassy-sync ^0.8 に依存しているので、版が合う。
  - **`I2cDeviceWithConfig` は M5 の機種で特に重要になる。** 同じバス上でデバイスごとにクロックが違うためである（M5Unified では ES8311 が 100kHz、それ以外が 400kHz）。

## 10. M5Unified / M5GFX の機種判別と、ハードウェアの事実

- **機種判別は M5GFX 側で行い、M5Unified はその結果を使うだけ。**
  - `M5Unified::begin()` は `Display.init()` を呼び、その後、判別に失敗した場合の代替候補を `cfg.fallback_board` → `getCandidateBoard()` → `_default_fallback_board()` の順に試す。
  - `_default_fallback_board()` は eFuse のパッケージ種別と rev を見る。
  - <https://github.com/m5stack/M5Unified/blob/master/src/M5Unified.hpp>
  - `src/M5Unified.inl`（1526 行付近）
- **最新の M5GFX は `src/board_detect/` と `extras/board_spec/*.json` に移行した。**
  - 機種ごとの情報を **JSON で記述し、そこからコードを生成**する。
  - `detection_package.inl` で、eFuse のパッケージ種別ごとに判別手順の候補を選ぶ。
  - 判別手順の例
    - PMIC の ID: `(0x34, reg 0x03) == 0x03` なら AXP192、`== 0x4A` なら AXP2101
    - タッチ IC が ACK を返すか: 0x38 なら Core2、0x2E なら Tough
    - パネル ID: RDDID
    - CoreS3 と CoreS3-SE の区別: カメラ（SCCB）の ID
  - <https://github.com/m5stack/M5GFX/tree/master/src/board_detect>
  - **ハードウェア記述層を設計するうえで最も近い先例**である（[07](07-design-hwdesc.md)）。
- ピンの表: `src/M5Unified.inl` の 100〜282 行（内部 I2C、外部 I2C、I2S、LED のピン）。

### 主要機種の構成（docs.m5stack.com より）

| 機種 | SoC | PMIC / IO | 表示 | タッチ | IMU | RTC | オーディオ | 出典 |
|---|---|---|---|---|---|---|---|---|
| Core2 | ESP32-D0WDQ6-V3 | AXP192 | ILI9342C/E | FT6336U 0x38 | MPU6886 0x68 | BM8563 0x51 | NS4168（EN は AXP IO2）、SPM1423 | <https://docs.m5stack.com/en/core/Core2> |
| Core2 v1.1 | 同上 | **AXP2101**, INA3221 | 同上 | 同上 | 同上 | 同上 | NS4168（EN は ALDO3） | <https://docs.m5stack.com/en/core/Core2%20v1.1> |
| CoreS3 | ESP32-S3 | AXP2101 0x34、AW9523B 0x58 | ILI9342C/E | FT6336U 0x38 | BMI270 **0x69**、BMM150 0x10 | BM8563 0x51 | AW88298 0x36、ES7210 0x40 | <https://docs.m5stack.com/en/core/CoreS3> |
| AtomS3 | ESP32-S3FN8 | なし | GC9107 128×128 | なし | MPU6886 0x68 | なし | なし | <https://docs.m5stack.com/en/core/AtomS3> |
| StickC Plus2 | ESP32-PICO-V3-02 | **PMIC なし**（GPIO4 で電源を保持） | ST7789V2 135×240 | なし | MPU6886 | BM8563 | ブザー（G2） | <https://docs.m5stack.com/en/core/StickC-Plus2> |
| Cardputer | Stamp-S3 | なし | ST7789V2 240×135 | なし | なし（ADV は BMI270） | なし | NS4168（ADV は ES8311） | <https://docs.m5stack.com/en/core/Cardputer> |
| PaperS3 | ESP32-S3R8 | PMS150G | ET047TC1 960×540（8bit パラレル） | GT911 | BMI270 0x68 | BM8563 | なし | <https://docs.m5stack.com/en/core/PaperS3> |
| Tab5 | ESP32-P4NRW32（+C6） | PI4IOE5V6408 0x43/0x44、INA226 | DSI 1280×720。ILI9881C（〜2025-10-14）→ ST7123 → ST7121（2026-04-28〜） | GT911 0x14、または ST7123 内蔵 0x55 | BMI270 0x68 | RX8130CE 0x32 | ES8388 0x10、ES7210 0x40 | <https://docs.m5stack.com/en/core/Tab5> |

- **同じ製品名でもロットによって部品が変わる**（Core2 の PMIC、ILI9342C と ILI9342E、Tab5 のパネル）。ビルド時の設定だけでは対応しきれず、実行時の判別が欠かせない理由である。
- CoreS3 の AW9523 のピン割り当て: LCD のリセットは P1_1、タッチのリセットと割り込みは P0_0 / P1_2、AW88298 のリセットは P0_2、カメラのリセットは P1_0。

## まとめ: 新しく書く必要があるドライバ

IP5306、AW88298、ES8388、BMM150、SH200Q、LTR-553、RX8130CE、PI4IOE5V6408、CST820、ST7123/ILI9881C（DSI）、MPU6886（eh1 版）、PaperS3 の EPD（寛容なライセンスのもの）。
