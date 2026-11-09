# Third-party notices

## semver-php

Composer constraint selection uses the MIT-licensed
[`semver-php` 0.1.0](https://crates.io/crates/semver-php/0.1.0)
dependency, a Rust port of Composer's constraint and interval logic.
It is linked as a Cargo dependency; its source is not vendored here.

## Mago

The locally patched `mago-syntax` and `mago-formatter` 1.58.2 sources in
[`third_party/mago/`](third_party/mago/README.md) are from
[Mago](https://github.com/carthage-software/mago), copyright 2024-present
Seifeddine Gmati. They are dual-licensed under MIT or Apache 2.0; the
upstream license texts and patch provenance are included in that directory.

## nikic/PHP-Parser

The built-in type introduction table in `src/types/php_version.rs` is
adapted from `BUILTIN_TYPE_VERSIONS` in
[`lib/PhpParser/PhpVersion.php`](https://github.com/nikic/PHP-Parser/blob/679d41ede0ad4236c22dbb52155aaa1b159b4d5c/lib/PhpParser/PhpVersion.php).
The adapted code is distributed under the following license:

```text
BSD 3-Clause License

Copyright (c) 2011, Nikita Popov
All rights reserved.

Redistribution and use in source and binary forms, with or without
modification, are permitted provided that the following conditions are met:

1. Redistributions of source code must retain the above copyright notice, this
   list of conditions and the following disclaimer.

2. Redistributions in binary form must reproduce the above copyright notice,
   this list of conditions and the following disclaimer in the documentation
   and/or other materials provided with the distribution.

3. Neither the name of the copyright holder nor the names of its
   contributors may be used to endorse or promote products derived from
   this software without specific prior written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS "AS IS"
AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE
IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE COPYRIGHT HOLDER OR CONTRIBUTORS BE LIABLE
FOR ANY DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL
DAMAGES (INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE
OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
```

## PHPStan

The `supports_promoted_properties` and `supports_property_hooks` checks
in `src/types/php_version.rs` are Rust adaptations of PHPStan's
`supportsPromotedProperties` and `supportsPropertyHooks` methods in
[`src/Php/PhpVersion.php`](https://github.com/phpstan/phpstan-src/blob/7c9daa4ca480f7407d3ce01754ea3a9a6d9b480c/src/Php/PhpVersion.php).
The surrounding integration and tests are specific to PHPantom.

The adapted code is distributed under the following license:

```text
MIT License

Copyright (c) 2016 Ondřej Mirtes
Copyright (c) 2025 PHPStan s.r.o.

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
