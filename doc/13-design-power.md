# 13. 電源管理の設計（検討、2026-11-10）

kumiki に電源管理を入れるための検討である。後から足すと、ドライバ・生成コード・API のすべてに手が入る。そこで、**今の段階で骨組みだけを決めて入れておくもの**と、**後で実装するもの**とを分ける。

## 1. 今の kumiki の状態

電源管理と呼べる仕組みは、まだない。

| 項目 | 今の状態 |
|---|---|
| `kumiki-class` の `Power` | `battery_mv()` と `vbus_present()` だけ。電源オフ、スリープ、復帰要因はない |
| レギュレータ | AXP2101 のレールは DT の `regulator-boot-on` で立ち上げるだけ。誰が使っているかの数え上げ（参照カウント）も、切る手段もない。[08](08-design-bsp.md) §2.2 の `Rail` は未実装 |
| デバイスの休止 | `Display` に `sleep` はない（`kumiki-gfx-panels` の `Dcs::sleep` はあるが、クラスの API になっていない）。コーデック、IMU、タッチにもない |
| 初期化の順序 | dtgen が DT の依存グラフから `INIT_ORDER` を作っている。逆順（休止の順序）はまだ出していない |
| 終了処理 | ほぼない。`Drop` を実装しているのは `kumiki-gfx-bus-esp` の `Fifo` だけ。P4 の HAL も、クロックを入れたままにしている |
| CPU の待機 | 同期版のデモは、ビジーループでポーリングしている。async 版は executor が WFI で待つ |
| 起動時の前提 | SoC をリセットしても、外のチップ（IMU、RTC、コーデック）や、AHB-GDMA・I2S は止まらないことを、これまでに何度も踏んだ（I2C のバスリカバリ、`halt_stale`、ES8388 の電源の入れ直し）。ただし、まだ個別の対処で、設計上の規則にはなっていない |
| 危険な線 | Tab5 の IO エキスパンダ #2 の bit4（電源が落ちる）は、DT で「決して書かない」としている。しかし、この線は電源オフに使う正規の手段でもある（M5Unified は 50 ms ごとに 10 回反転させて電源を切る） |

[08](08-design-bsp.md) §5 には、`Power` の機能（電源オフ、ディープスリープと復帰要因、`Rail`）と危険な線の保護が書いてある。ただし、「誰がいつ休止させるか」「順序」「起こす要因」「起動時に前の状態をどう扱うか」は決めていない。

## 2. ほかの OS の作り

### 2.1 Linux

- **システムの休止**（suspend-to-idle、suspend-to-RAM、hibernate）: 全デバイスの `dev_pm_ops` の `suspend` → `suspend_late` → `suspend_noirq` を、**登録順の逆**に呼ぶ。復帰は同じ順で `resume_*` を呼ぶ。親子関係や device link で、順序の制約を足せる。
- **ランタイム PM**: ドライバが `pm_runtime_get()` / `put()` で使用数を増減し、0 になってから `autosuspend_delay` が過ぎると `runtime_suspend` が呼ばれる。親は、子がすべて休止するまで休止しない。
- **レギュレータ**: 消費者ごとに `regulator_enable()` / `disable()` し、使用数で実際にオン・オフする。DT の `regulator-always-on`（切らない）と `regulator-boot-on`（起動時にオンだったとみなす）がある。
- **電源ドメイン**（genpd）: DT の `power-domains` で参照し、ドメインの中の全デバイスが休止すると落とす。
- **起こす要因**: DT の `wakeup-source` と、`device_may_wakeup()` / `enable_irq_wake()`。
- **電源オフ**: PMIC ノードの `system-power-controller`、または `gpio-poweroff` / `regulator-poweroff` ノード。`pm_power_off` に登録された 1 つが、最後に呼ばれる。

### 2.2 Zephyr

- **デバイス PM**: ドライバは、`pm_device_action_cb_t(dev, action)` 1 つで `SUSPEND` / `RESUME` / `TURN_ON` / `TURN_OFF` を受け取る。状態は `ACTIVE` / `SUSPENDING` / `SUSPENDED` / `OFF`。状態の遷移は PM サブシステムが管理し、コールバックの呼び出しも直列化される。
  - 初期化関数の最後に `pm_device_driver_init()` を呼び、初期状態（`TURN_ON` → `RESUME`）へ移す。
- **ランタイム PM（推奨）**: `pm_device_runtime_get()` / `put()` / `put_async()` で使用数を数える。
  - 親（バス）の get/put は、自動では行わない。必要ならドライバが自分で呼ぶ。
  - DT の `power-domains` で参照する電源ドメインだけは、子の get/put に連動して入り切りする。
  - DT の `zephyr,pm-device-runtime-auto` で、デバイスごとに有効にする。
- **システム PM**: アイドルスレッドが `pm_system_suspend()` を呼ぶ。
  - 次のタイマーまでの時間が `min-residency-us` と `exit-latency-us` を超える、いちばん深い状態を選ぶ（DT の `zephyr,power-state`）。
  - `pm_policy_state_lock_get()` で、特定の状態に入るのを止められる。
  - `CONFIG_PM_DEVICE_SYSTEM_MANAGED` では、深い状態に入る前に全デバイスを初期化の順に休止し、復帰は逆の順に行う。ただし、文書はランタイム PM の方を推している。
- **その他**: `pm_device_busy_set()`（処理中は休止させない）、DT の `wakeup-source` と `pm_device_wakeup_enable()`、`zephyr,deferred-init`。

### 2.3 その他の RTOS と SDK

| 対象 | 仕組み |
|---|---|
| ESP-IDF | `esp_pm_lock`（`CPU_FREQ_MAX` / `APB_FREQ_MAX` / `NO_LIGHT_SLEEP`）の参照カウント。誰もロックを持っていなければ、FreeRTOS の tickless idle から自動でライトスリープに入る。ドライバが自分でロックを取る（SPI マスタと I2C は転送中だけ、GPTimer などは有効な間ずっと、I2S は APLL を使うとき `NO_LIGHT_SLEEP`）。P4 では、ライトスリープ中に周辺回路の電源を落とし、REG_DMA でレジスタを退避・復元できる（対応していない周辺回路もある） |
| NuttX | ドライバが `pm_register()` で `prepare`（遷移してよいかを聞く。拒否できる）と `notify`（遷移の通知）を登録する。`pm_activity()` でドメインごとの活動を報告し、ガバナー（greedy / stability / activity）が状態を選ぶ |
| RIOT | `pm_layered`: 各モードを `pm_block()` / `pm_unblock()` で参照カウントする。アイドルスレッドは、止められていない中でいちばん深いモードに入る。あるモードを止めると、それより深いモードもすべて止まる |
| embassy（stm32） | 低電力用の executor が、次のタイマーまでが `min_stop_pause`（既定 250 ms）以上なら STOP モードに入り、RTC で起きる。STOP を妨げる周辺回路は、使っている間それを止める（`DeviceBusy`）。デバイスの休止の仕組みは持たない |
| esp-hal 1.2 | `LowPower` の `sleep_light` / `sleep_deep`（起こす時刻を指定する）、`reset_reason()`、`wakeup_cause()`。周辺回路のクロックは `PeripheralGuard` が参照カウントし、`Drop` で止める |
| M5Unified | `Power.powerOff()` / `deepSleep()` / `lightSleep()` / `timerSleep()`。機種ごとの `switch` で、PMIC のオフ、電源保持 GPIO のパルス、Tab5 の PWROFF パルス、RTC のアラームによる起動を切り替える。どれも、最初に `Display.sleep()` を呼ぶだけで、ほかのデバイスは休止させない |

### 2.4 共通して見られること

1. **参照カウント**: 「使っている間は切らない」を、使用数（Linux・Zephyr のランタイム PM、レギュレータ）や、深いモードを止めるロック（ESP-IDF、RIOT、embassy）で表す。グローバルな判断を、各ドライバの局所的な get/put に分解している。
2. **順序は依存グラフから決める**: 休止は初期化の逆順、復帰は初期化の順にする。
3. **ハードウェアの事情は DT に書く**: `wakeup-source`、`power-domains`、`*-supply`、`regulator-always-on`、`system-power-controller`、`gpio-poweroff`、CPU の電源状態とその遅延。
4. **MCU 向けの OS ほど、システム全体を一斉に休止させない**: Zephyr はランタイム PM を推し、ESP-IDF・RIOT・embassy は「誰も止めていなければ深く眠る」という形だけを持つ。全デバイスを一斉に休止させるのは、Linux のシステムの休止くらいである。

## 3. ハードウェアの事情

### 3.1 SoC の電源状態

| 段階 | ESP32-S3 | ESP32-P4 | RAM | 起きるまで |
|---|---|---|---|---|
| CPU の待機（WFI / waiti） | ○ | ○ | 保持 | 割り込みですぐ |
| ライトスリープ | ○ | ○（周辺回路の電源を落とすと、レジスタの退避と復元が要る） | 保持 | 約 1 ms。タイマー、GPIO、UART などで起きる |
| ディープスリープ | ○（RTC メモリと RTC GPIO は保持） | ○（LP メモリと LP GPIO は保持） | **失う** | **リセットと同じ**。起動処理が最初から走る。S3 は RTC IO（GPIO0〜21）、P4 は LP IO（0〜15）で起こせる |
| 電源オフ（PMIC / 電源保持） | 機種による | 機種による | 失う | 電源キーか、RTC のアラーム（RTC の INT が PMIC の電源投入につながっている機種） |

### 3.2 ボードの事情

- **外のチップは SoC と電源が別である**: SoC をリセットしても、ディープスリープから起きても、IMU、RTC、コーデック、タッチは前の状態のまま残る。ES8388 の件（[12](12-m4-tab5-plan.md)）は、電源の入れ直しそのものが直し方になった例である。
- **レールの共有**: CoreS3 の AXP2101 には、ALDO1（AW88298）、ALDO2（ES7210）、ALDO3（カメラ）、ALDO4（SD）、DLDO1（バックライト）がある。ほかに、AW9523 経由の BOOST_EN（5V）もある。Tab5 は、IO エキスパンダの線でアンプ、外部 5V、リセットを切り替える。StopWatch は、M5PM1 と M5IOE1 のレジスタ手順で電源を入れる。
- **電源オフの手段は機種ごとに違う**: AXP2101 のレジスタ、M5PM1 のレジスタ、電源保持 GPIO のパルス、Tab5 の PWROFF パルス（エキスパンダ #2 の bit4）がある。
- **起こす要因の配線**: タッチの INT、電源キー、RTC の INT がどこにつながっているか。RTC IO / LP IO でなければ、ディープスリープからは起こせない。

## 4. 設計方針（案）

### 4.1 基本の形: 局所的な参照カウントと、深いモードを止めるガード

kumiki の `Devices` は、フィールドごとに分けてアプリが所有する（[08](08-design-bsp.md) §2.1）。そのため、Linux のように、中央の一覧から全デバイスを一斉に休止させる形とは相性が悪い。Rust の所有権に合い、MCU 向けの OS でも主流の、次の 3 つを組み合わせる。

1. **デバイスの休止は、デバイスの持ち主が行う**（ランタイム PM の考え方）。クラスのトレイトに、休止と復帰を足す（§4.2）。
2. **共有する資源（レール、電源ドメイン、エキスパンダの線）は、参照カウントで入り切りする**。Linux のレギュレータと Zephyr の power-domains と同じである。DT の `*-supply` から、各デバイスに `RailHandle` を渡す。使用数が 0 になったら切る。`regulator-always-on` は切らない。
3. **システムのスリープは、誰も止めていなければ入る**。RAII の `SleepGuard`（RIOT の `pm_block`、ESP-IDF の `esp_pm_lock`、embassy の `DeviceBusy` に相当）が、指定した深さより深いスリープを止める。
   - 例: I2S のストリーミング中は、ライトスリープを止める。表示の DMA 転送中も同様。
   - スリープに入る関数は、止められていれば `Err(Blocked)` を返す。何も起きないまま黙って失敗することはしない。

一斉の休止は、**電源オフとディープスリープのときだけ**、生成コードの関数で行う（§4.5）。このときはどちらにしても RAM を失うので、所有権を `Devices` ごと受け取ってよい。

### 4.2 クラスの API

```rust
/// Zephyr の pm_device_action に相当する。
pub enum PmAction { Suspend, Resume }

pub trait DevicePm {
    type Error: core::fmt::Debug;
    /// 休止 / 復帰する。休止中に、ほかのメソッドを呼んだときの動作はクラスごとに決める。
    fn pm(&mut self, action: PmAction) -> Result<(), Self::Error>;
}
```

- **既定の実装は与えない**。何もしないデバイスは、バインディングに `pm: none` と書く。dtgen は、それを見て何もしない実装を生成する。暗黙の空実装があると、休止したつもりで電流が流れ続ける（Zephyr が `-ENOTSUP` を返させるのと同じ考え方）。
- 各クラスの休止の中身
  - `Display`: 画面を消す → パネルを SLPIN → バックライトのレールを手放す。復帰は、この逆順に行う。
  - `Speaker`: アンプを切る → コーデックの電源を落とす（ES8388 の `power_down`）。復帰は `power_up` で行う。今の `Speaker::restart` は、`Suspend` と `Resume` の組で置き換えられる。
  - `Microphone`、`Imu`、`Touch`: それぞれのチップの低電力モードを使う。
- **スリープを止める状態は、デバイス自身が `SleepGuard` を持つことで表す**。例えば、再生中の `Speaker` はガードを持ち、`Suspend` で手放す。

### 4.3 DT とバインディング

Linux / Zephyr の標準プロパティを、そのまま使う。

| プロパティ | 意味 | dtgen が生成するもの |
|---|---|---|
| `*-supply`（既存） | 使うレール | 参照カウントのある `RailHandle` を渡す |
| `regulator-always-on` | 切らないレール | ハンドルを渡さず、常にオンにする |
| `power-domains` | まとめて入り切りする単位（エキスパンダの線など） | レールと同じ扱い |
| `wakeup-source` | 起こす要因になれるデバイス | 起こす要因の一覧。**INT の線が RTC IO / LP IO でなければ、生成時にエラーにする**（ディープスリープからは起こせないため） |
| `system-power-controller`（PMIC ノード） | 電源オフを担うノード | `power_off()` の実装 |
| `gpio-poweroff`（`gpios`、`active-delay-ms` など） | GPIO で電源を切る | 同上。Tab5 は、エキスパンダ #2 の bit4 をこのノードにだけ渡す |
| `kumiki,protected-lines`（既存の案） | 誰にも渡さない線 | `gpio-poweroff` が参照する線だけは、そのノードにだけ渡す |

- 休止の順序: dtgen は、`INIT_ORDER` に加えて、その逆順（`SUSPEND_ORDER`）を出す。
- `kumiki,deferred-init`（[08](08-design-bsp.md) §4 の案）は、「起動時は休止させたままにしておく」と同じ意味になる。

### 4.4 起動時の規則（今すぐ決めるもの）

これまでに踏んだ件を、規則にする。ディープスリープからの起床もリセットと同じ経路を通るので、この規則がそのまま「ディープスリープからの復帰」の土台になる。

1. **ドライバの `init` は、どの状態からでも正しく始められること**。電源投入直後の状態を仮定しない。必要なら、ソフトリセットか電源の入れ直しを、自分の手順に含める。
2. **HAL の起動処理は、前のプログラムが動かしたままの DMA やストリームを止める**（`halt_stale` の一般化）。P4 では、DSI と AHB-GDMA が対象になる。
3. **`Devices` に `boot: BootInfo { reset_reason, wakeup_cause }` を持たせる**。アプリと生成コードが、電源投入（コールドブート）と、ディープスリープからの起床（ウォームブート）を区別できるようにする。
   - ウォームブートでは、画面の消去や起動音を省ける。
   - RTC / LP メモリに置いた値（`#[link_section = ".rtc.data"]` など）も使える。

### 4.5 電源オフとディープスリープ

```rust
impl Devices {
    /// SUSPEND_ORDER の順に全デバイスを休止させ、DT の電源オフ手段で切る。
    pub fn power_off(self) -> !;
    /// 起こす要因を設定してディープスリープに入る（RAM を失う）。
    pub fn deep_sleep(self, wake: WakeConfig) -> !;
}
```

- `self` を受け取るので、アプリが一部のフィールドを取り出して使っていると呼べない。その場合のために、各フィールドを `Option` で返してもらう版（`Devices::reassemble(parts)`）か、生成された休止関数を、フィールドごとに呼ぶ版を用意する。どちらにするかは、§6 の未決事項とする。
- タイマーで起きる電源オフ（M5Unified の `timerSleep`）: RTC にアラームを設定してから `power_off` する。RTC の INT が PMIC の電源投入につながっていることを、DT に書く。

### 4.6 自動のライトスリープと周波数の切り替え（後で）

ESP-IDF の自動ライトスリープに相当する。次の時刻（`Clock` の次のアラーム）まで十分に時間があり、`SleepGuard` が誰もいなければ、アイドル時にライトスリープに入る。

- P4 で周辺回路の電源を落とすなら、REG_DMA によるレジスタの退避と復元が要る。手間が大きいので、後回しにする。
- 周波数の切り替え（DFS）も後で行う。APB の周波数に依存する周辺回路（I2C、SPI のクロック分周）の扱いが要るので、それまでは固定周波数のままとする。

## 5. 段階分け

| 段階 | 内容 | 今入れておく理由 |
|---|---|---|
| **今** | §4.4 の起動時の規則（`BootInfo`、ドライバの規則を文書にすること） | 後から入れると、全ドライバを見直すことになる。踏んだ件がすでに 3 つある |
| **今** | `DevicePm` トレイトと、バインディングの `pm:` 欄。dtgen の `SUSPEND_ORDER` | トレイトとバインディングの形が決まっていれば、ドライバを書くたびに休止の手順も一緒に書ける。後から足すのが、いちばん面倒な部分 |
| **今** | `Rail` の参照カウントと、`*-supply` から `RailHandle` を渡す生成 | 今はレールを立ち上げるだけなので、まだ変更が小さい |
| **今** | DT の `wakeup-source`、`system-power-controller`、`gpio-poweroff` の解釈と検査 | Tab5 の PWROFF の線を、「書かない線」から「電源オフのノードだけが使う線」に直せる |
| 次 | `power_off()`、`deep_sleep()`、RTC のアラームで起動 | 3 機種で実際に電流を測りながら行う |
| 次 | `SleepGuard` と、手動のライトスリープ | I2S と表示の DMA がガードを持つ |
| 後 | 自動のライトスリープ、DFS、P4 の周辺回路の電源オフとレジスタの退避 | 必要になったときに行う |

## 6. 未決事項

- `power_off` / `deep_sleep` を、分割して所有しているデバイスに対してどう呼ばせるか（§4.5）。
- `DevicePm` を同期版だけにするか、async 版も別トレイトで持つか（[08](08-design-bsp.md) §2.2 の方針どおりなら、両方持つ）。
- ライトスリープ中に表示を残すか（パネルは GRAM を保持するので、バックライトだけ落とすこともできる）。これはアプリの方針なので、API では選べるようにする。
- USB-Serial-JTAG は、スリープ中に切れる。開発中のログをどうするか。

## 出典

- Zephyr: [Device Runtime Power Management](https://docs.zephyrproject.org/latest/services/pm/device_runtime.html)、[System Power Management](https://docs.zephyrproject.org/latest/services/pm/system.html)、[Device Power Management](https://docs.zephyrproject.org/latest/services/pm/device.html)
- Linux: [Device Power Management Basics](https://www.kernel.org/doc/html/latest/driver-api/pm/devices.html)
- ESP-IDF: [Power Management（ESP32-P4）](https://docs.espressif.com/projects/esp-idf/en/latest/esp32p4/api-reference/system/power_management.html)
- NuttX: [Power Management](https://nuttx.apache.org/docs/latest/implementation/power_management.html)、[Power Management in NuttX（Espressif）](https://developer.espressif.com/blog/2026/02/power-management-in-nuttx/)
- RIOT: [Layered PM Infrastructure](https://doc.riot-os.org/group__sys__pm__layered.html)
- embassy: [embassy_stm32::low_power](https://docs.embassy.dev/embassy-stm32/git/stm32g491mc/low_power/index.html)
- esp-hal 1.2.2 の `rtc_cntl/sleep`、`system.rs`（`PeripheralGuard`）
- M5Unified 0.2.13 の `Power_Class.cpp`（`_powerOff`、`deepSleep`、`lightSleep`）
