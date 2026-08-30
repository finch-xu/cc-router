# 第三方组件声明 (Third-Party Notices)

cc-router 本体以 MIT 许可证发布 (见 [LICENSE](LICENSE))。本文件声明项目使用的第三方
组件及其许可证。

> 字体部分由 `scripts/collect-receipt-fonts.mjs` 从 `node_modules` 生成, 请勿手工编辑。
> 许可证正文保留英文原文 —— 译本不具法律效力。

## 字体 (Fonts)

小票主题 (`src/components/receipts/themes/`) 使用以下 9 款字体,
**全部为 SIL Open Font License 1.1 (OFL-1.1)**, 均取自 [Google Fonts](https://github.com/google/fonts)
经由 [Fontsource](https://fontsource.org) 分发。

| 字体 | 版权声明 |
| --- | --- |
| Archivo | Copyright 2020 The Archivo Project Authors (https://github.com/Omnibus-Type/Archivo) |
| Archivo Narrow | Copyright 2019 The Archivo Narrow Project Authors (https://github.com/Omnibus-Type/ArchivoNarrow) |
| Caveat | Copyright 2014 The Caveat Project Authors (https://github.com/googlefonts/caveat) |
| Cormorant Garamond | Copyright 2015 The Cormorant Project Authors (github.com/CatharsisFonts/Cormorant) |
| Courier Prime | Copyright 2015 The Courier Prime Project Authors (https://github.com/quoteunquoteapps/CourierPrime). |
| DotGothic16 | Copyright 2020 The DotGothic16 Project Authors (https://github.com/fontworks-fonts/DotGothic16/) |
| IBM Plex Mono | Copyright 2017 IBM Corp. All rights reserved. |
| Oswald | Copyright 2016 The Oswald Project Authors (https://github.com/googlefonts/OswaldFont) |
| Space Mono | Copyright 2016 The Space Mono Project Authors (https://github.com/googlefonts/spacemono) |

**当前分发方式**: 字体文件既不在本仓库中, 也不随安装包分发 —— `src/receipt-fonts.css`
的 `@font-face` 仅引用公共 CDN (jsDelivr 主源 / npmmirror 兜底), 由 WebView 按需加载,
离线时回退系统字体; 导出的小票 HTML 同样只内嵌 `@font-face` 规则而非字体数据。
`@fontsource/*` 只列在 `devDependencies`, 供上述生成脚本读取, 不进构建产物。

保留本声明有两个目的: (a) 向字体作者署名; (b) 若将来把 woff2 打进安装包 (例如为了
离线可用), 那一刻起即构成 OFL 意义上的字体再分发, 其第 2 条要求随附版权声明与许可证
全文 —— 本文件届时已满足该要求, 无需再补。

以上字体**均未声明 Reserved Font Name (RFN)**, 因此衍生版本 (例如为减小体积做字符
子集化) 可继续使用原字体名。

OFL 同时明确豁免了用字体产出的文档 (见正文 PREAMBLE 末句): 用户从 cc-router 导出的
小票 HTML / PNG / PDF 不受 OFL 约束。

## 许可证全文 (OFL-1.1)

以下正文为上述 9 款字体共用, 逐字取自 `node_modules/@fontsource/*/LICENSE`。

```
-----------------------------------------------------------
SIL OPEN FONT LICENSE Version 1.1 - 26 February 2007
-----------------------------------------------------------

PREAMBLE
The goals of the Open Font License (OFL) are to stimulate worldwide
development of collaborative font projects, to support the font creation
efforts of academic and linguistic communities, and to provide a free and
open framework in which fonts may be shared and improved in partnership
with others.

The OFL allows the licensed fonts to be used, studied, modified and
redistributed freely as long as they are not sold by themselves. The
fonts, including any derivative works, can be bundled, embedded,
redistributed and/or sold with any software provided that any reserved
names are not used by derivative works. The fonts and derivatives,
however, cannot be released under any other type of license. The
requirement for fonts to remain under this license does not apply
to any document created using the fonts or their derivatives.

DEFINITIONS
"Font Software" refers to the set of files released by the Copyright
Holder(s) under this license and clearly marked as such. This may
include source files, build scripts and documentation.

"Reserved Font Name" refers to any names specified as such after the
copyright statement(s).

"Original Version" refers to the collection of Font Software components as
distributed by the Copyright Holder(s).

"Modified Version" refers to any derivative made by adding to, deleting,
or substituting -- in part or in whole -- any of the components of the
Original Version, by changing formats or by porting the Font Software to a
new environment.

"Author" refers to any designer, engineer, programmer, technical
writer or other person who contributed to the Font Software.

PERMISSION & CONDITIONS
Permission is hereby granted, free of charge, to any person obtaining
a copy of the Font Software, to use, study, copy, merge, embed, modify,
redistribute, and sell modified and unmodified copies of the Font
Software, subject to the following conditions:

1) Neither the Font Software nor any of its individual components,
in Original or Modified Versions, may be sold by itself.

2) Original or Modified Versions of the Font Software may be bundled,
redistributed and/or sold with any software, provided that each copy
contains the above copyright notice and this license. These can be
included either as stand-alone text files, human-readable headers or
in the appropriate machine-readable metadata fields within text or
binary files as long as those fields can be easily viewed by the user.

3) No Modified Version of the Font Software may use the Reserved Font
Name(s) unless explicit written permission is granted by the corresponding
Copyright Holder. This restriction only applies to the primary font name as
presented to the users.

4) The name(s) of the Copyright Holder(s) or the Author(s) of the Font
Software shall not be used to promote, endorse or advertise any
Modified Version, except to acknowledge the contribution(s) of the
Copyright Holder(s) and the Author(s) or with their explicit written
permission.

5) The Font Software, modified or unmodified, in part or in whole,
must be distributed entirely under this license, and must not be
distributed under any other license. The requirement for fonts to
remain under this license does not apply to any document created
using the Font Software.

TERMINATION
This license becomes null and void if any of the above conditions are
not met.

DISCLAIMER
THE FONT SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND,
EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO ANY WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT
OF COPYRIGHT, PATENT, TRADEMARK, OR OTHER RIGHT. IN NO EVENT SHALL THE
COPYRIGHT HOLDER BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
INCLUDING ANY GENERAL, SPECIAL, INDIRECT, INCIDENTAL, OR CONSEQUENTIAL
DAMAGES, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF THE USE OR INABILITY TO USE THE FONT SOFTWARE OR FROM
OTHER DEALINGS IN THE FONT SOFTWARE.
```

## 其他依赖

前端 npm 依赖见 [`package.json`](package.json) 与 `pnpm-lock.yaml`, Rust 依赖见
[`src-tauri/Cargo.toml`](src-tauri/Cargo.toml) 与 `Cargo.lock`; 各自许可证随包分发,
未在此逐一转录。
