//! Power-management checks of the generator (doc/13): wake-up sources,
//! power-off providers, the lines a power-off owns.

use std::path::{Path, PathBuf};

use kumiki_dtgen::{power_off, validate, wakeup_sources, Bindings, Board, Gpio, PowerOff};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap()
}

struct T<'a> {
    /// The PMIC is the `system-power-controller`.
    pmic_powers_off: bool,
    /// Extra nodes in `/`.
    extra_root: &'a str,
    /// Extra children of the expander.
    extra_exp: &'a str,
    /// GPIO of the wake-up button.
    key_pin: u32,
}

impl Default for T<'_> {
    fn default() -> Self {
        Self { pmic_powers_off: true, extra_root: "", extra_exp: "", key_pin: 3 }
    }
}

/// A CoreS3-like ESP32-S3 board: AXP2101, AW9523 expander, one button.
fn board(name: &str, t: T) -> Board {
    let root = root();
    let spc = if t.pmic_powers_off { "system-power-controller;" } else { "" };
    let dts = format!(
        r#"/dts-v1/;
#include <soc/espressif/esp32s3.dtsi>
#include <kumiki/gpio.h>
/ {{
	model = "Test";
	compatible = "test,board", "espressif,esp32s3";
	buttons {{
		compatible = "gpio-keys";
		button-a {{ gpios = <&gpio {} GPIO_ACTIVE_LOW>; kumiki,button = "a"; wakeup-source; }};
	}};
	{}
}};
&i2c0 {{
	status = "okay";
	sda-gpios = <&gpio 12 GPIO_ACTIVE_HIGH>;
	scl-gpios = <&gpio 11 GPIO_ACTIVE_HIGH>;
	pmic: pmic@34 {{ compatible = "x-powers,axp2101"; reg = <0x34>; {spc} }};
	ioexp: gpio@58 {{
		compatible = "awinic,aw9523b"; reg = <0x58>; gpio-controller; #gpio-cells = <2>;
		{}
	}};
}};
"#,
        t.key_pin, t.extra_root, t.extra_exp
    );
    let dir = std::env::temp_dir().join(format!("kumiki-power-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.dts"));
    std::fs::write(&path, dts).unwrap();
    let b = Board::load(&path, &[root.join("boards/dts"), root.join("boards/dts/include")]).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    b
}

fn errors(b: &Board) -> Vec<String> {
    validate(b, &Bindings::load_dir(&root().join("boards/bindings")).unwrap())
}

const GPIO_POWEROFF: &str = r#"poweroff { compatible = "gpio-poweroff"; gpios = <&ioexp 7 GPIO_ACTIVE_HIGH>; kumiki,pulses = <5>; };"#;

#[test]
fn the_test_board_is_valid() {
    let b = board("valid", T::default());
    assert_eq!(errors(&b), Vec::<String>::new());
}

#[test]
fn wakeup_source_on_an_rtc_io_wakes_from_deep_sleep() {
    let w = wakeup_sources(&board("rtc", T::default())).unwrap();
    assert_eq!(w.len(), 1);
    assert_eq!((w[0].path.as_str(), w[0].pin, w[0].active_low, w[0].deep_sleep), ("/buttons/button-a", 3, true, true));
    // GPIO41 is not an RTC IO: light sleep only.
    let w = wakeup_sources(&board("digital", T { key_pin: 41, ..T::default() })).unwrap();
    assert!(!w[0].deep_sleep);
}

#[test]
fn wakeup_source_on_an_expander_line_is_rejected() {
    let extra = r#"k2 { compatible = "gpio-keys"; b { gpios = <&ioexp 3 GPIO_ACTIVE_LOW>; kumiki,button = "b"; wakeup-source; }; };"#;
    let b = board("exp", T { extra_root: extra, ..T::default() });
    assert!(wakeup_sources(&b).unwrap_err().0.contains("cannot wake the chip"));
    assert!(errors(&b).iter().any(|m| m.contains("cannot wake the chip")));
}

#[test]
fn wakeup_source_needs_a_line() {
    let extra = r#"thing { compatible = "kumiki,touch-keys"; touch = <&pmic>; wakeup-source; };"#;
    let b = board("noline", T { extra_root: extra, ..T::default() });
    assert!(errors(&b).iter().any(|m| m.contains("wakeup-source needs one of")), "{:?}", errors(&b));
}

#[test]
fn pmic_power_off() {
    assert_eq!(power_off(&board("pmic", T::default())).unwrap(), Some(PowerOff::Pmic { path: "/soc/i2c@60013000/pmic@34".into() }));
}

#[test]
fn gpio_poweroff_defaults_follow_linux() {
    let b = board("gpo", T { pmic_powers_off: false, extra_root: GPIO_POWEROFF, ..T::default() });
    assert_eq!(errors(&b), Vec::<String>::new());
    assert_eq!(
        power_off(&b).unwrap(),
        Some(PowerOff::Gpio {
            path: "/poweroff".into(),
            gpio: Gpio::Expander { controller: "ioexp".into(), pin: 7, flags: 0 },
            active_ms: 100,
            inactive_ms: 100,
            pulses: 5,
        })
    );
}

#[test]
fn one_power_off_provider_only() {
    let b = board("two", T { extra_root: GPIO_POWEROFF, ..T::default() });
    assert!(power_off(&b).unwrap_err().0.contains("more than one power-off provider"));
    assert!(errors(&b).iter().any(|m| m.contains("more than one power-off provider")));
}

#[test]
fn no_gpio_hog_on_a_power_off_line() {
    let hog = r#"bad-hog { gpio-hog; gpios = <7 GPIO_ACTIVE_HIGH>; output-low; };"#;
    let b = board("hog", T { pmic_powers_off: false, extra_root: GPIO_POWEROFF, extra_exp: hog, ..T::default() });
    assert!(errors(&b).iter().any(|m| m.contains("line 7 belongs to gpio-poweroff")), "{:?}", errors(&b));
}
