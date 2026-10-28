# 只内置可自由嵌入的字体

编辑器只内置可自由嵌入的字体（Noto Sans SC/Noto Serif SC——思源黑体/思源宋体的 TrueType 构建、Inter、Noto Serif、Liberation Sans/Serif），保存时按需子集嵌入；编辑已有文字时优先复用文档原字体，缺字形回退到内置字体并提示。使用系统字体选择器会让文档在不同机器上不可复现，且系统字体的嵌入许可不明确；固定字体集让包体、文件体积与跨平台渲染都可预期。

CJK 字体使用 TrueType 构建，以配合 CIDFontType2 + FontFile2 管线。Adobe 发布的思源黑体/宋体 OTF 是 CFF 字体，若改用它们，需要另行实现 CIDFontType0/CFF 与 CID 映射，不能与本管线混用。
