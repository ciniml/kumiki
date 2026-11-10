# Third-party notices

kumiki itself is licensed under MIT OR Apache-2.0. Some parts are ported from
or based on the following projects; their notices are reproduced here.

## Bosch Sensortec BMI270_SensorAPI

Used for: the BMI270 configuration blob. `drivers/kumiki-bmi270/vendor/bmi270.c`
is an unmodified copy of `bmi270.c` v2.86.1 from
<https://github.com/boschsensortec/BMI270_SensorAPI> and carries its own
BSD-3-Clause notice (Copyright (c) 2023 Bosch Sensortec GmbH). Binaries that
include `kumiki-bmi270` must reproduce that notice.

## M5GFX

Used for: panel init tables, rotation/MADCTL tables, board bring-up sequences
(`gfx/kumiki-gfx-panels/src/models.rs`, `gfx/kumiki-gfx-panels/src/dcs.rs`, firmware board init).

```
MIT License

Copyright (c) 2021 M5Stack

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
```

## M5Unified

Used for: RTC register use and start-up writes (`drivers/kumiki-rtc`, from
M5Unified 0.2.13 `src/utility/rtc/{PCF8563,RX8130}_Class.cpp`). Same MIT
license text as M5GFX above, Copyright (c) 2021 M5Stack.

## StackChan-BSP

Used for: the PY32 IO expander register map, line set-up and LED format
(`drivers/kumiki-py32`, from `src/drivers/PY32IOExpander`), and the Stack-chan
base facts in `boards/dts/m5stack/cores3-stackchan.dts` (from
`src/M5StackChan.cpp`). Same MIT license text as M5GFX above, Copyright (c)
2026 M5Stack Technology CO LTD.

## LovyanGFX

The M5GFX graphics core (`src/lgfx/v1`) is LovyanGFX, licensed under the
FreeBSD (BSD 2-Clause) license.

```
Copyright (c) 2020 lovyan03 (https://github.com/lovyan03)
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.
2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR
ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND
ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```
