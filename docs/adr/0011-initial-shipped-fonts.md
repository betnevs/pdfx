# 初始版本只内置 Noto Sans SC 与 PDF base-14 字体

初始发布只随包内置 Noto Sans SC（思源黑体的 TrueType 构建）作为 CJK 回退，并使用 PDF base-14 作为拉丁文字的基础字体；保存时按需做 Font Subset 嵌入。ADR-0006 列出的 Noto Serif SC、Inter、Noto Serif 与 Liberation Sans/Serif 推迟到后续版本。原因是当前只完成并验证了 Noto Sans SC 的 CIDFontType2 + FontFile2 子集管线，且只对该字体完成了包体、渲染一致性与再分发许可的复核；在没有同等验证前打包其余字体，会在不同平台产生缺字形或文件体积不可预期的风险。

这一取舍不改变 ADR-0006 的许可原则：编辑器仍只使用可自由嵌入的字体，不调用系统字体选择器，编辑已有文字时优先复用文档原字体，缺少字形时回退到内置字体并提示。后续加入其他字族时，必须复用同一条 TrueType 子集管线并单独完成许可复核，不能因为字体“常见”就默认可以随包分发。
