# 仿宋原字体缺字形时回退到朱雀仿宋

> 后续：[ADR-0014](0014-bundled-font-variety-and-original-font-restyle.md) 把朱雀仿宋加入文本框字体列表，并新增五款 OFL 字体。本 ADR 中“朱雀仿宋只作为回退字体，不出现在文本框字体列表中”的表述已被取代；仿宋类原字体的回退顺序仍然有效。

编辑仿宋文档时，ADR-0012 的按字符保留会保留原字体绘制的字符，但原嵌入子集缺字形的字符需要一个回退字体。此前回退到 Noto Serif SC，而宋体的横竖粗细与仿宋的细瘦笔画差异明显：即使只新增一两个字，编辑后的标签也会出现一段明显更重的文字。参考产品 iLovePDF 对同一文档的处理是保留原字符、仅缺字形字符改用 Noto Sans SC；这样做字形不突兀，但新字已不是仿宋。PDFX 选择更贴近原貌的做法：仿宋类原字体（FangS-SC、STFangsong、FangSong_GB2312、仿宋）缺字形时改用随包内置的朱雀仿宋（Zhuque Fangsong）Regular，其他衬线原字体仍用 Noto Serif SC，非衬线原字体仍用 Noto Sans SC。

朱雀仿宋（TrionesType，OFL 1.1，v0.212 技术预览版）是静态 TrueType 字体，已完成与 Noto 字体相同的 CIDFontType2 + FontFile2 子集管线验证，并纳入再分发许可复核。代价有三项，均已接受：包体增加约 8.8 MB；只有 Regular 一个实例，原字体为粗体时新字形不会加粗；字表为 GB2312 级别，少于 Noto 字体，因此回退顺序是朱雀仿宋 → Noto Serif SC → Noto Sans SC，逐级寻找能覆盖缺失字符的字族。朱雀仿宋只作为回退字体，不出现在文本框字体列表中。

这一决策是 [ADR-0012](0012-per-character-font-preservation.md) 中“按风格选择回退字族”的细化，不改变其按字符保留原字体的原则；[ADR-0011](0011-initial-shipped-fonts.md) 中“加入字族必须复用同一 TrueType 子集管线并单独完成许可复核”的要求同样适用于本字体。
