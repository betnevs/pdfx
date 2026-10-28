// Inspects a PDF with PDFKit (the engine behind macOS Preview) and writes a
// JSON report. Run: swift reader/pdfkit_probe.swift file.pdf [out-dir]
import Foundation
import PDFKit
import AppKit

func dictionary(_ annotation: PDFAnnotation) -> [String: Any] {
    var value: [String: Any] = [
        "type": annotation.type ?? "",
    ]
    if let name = annotation.fieldName { value["field_name"] = name }
    let field_type = annotation.widgetFieldType
    switch field_type {
    case .text: value["field_type"] = "text"
    case .button: value["field_type"] = "button"
    case .choice: value["field_type"] = "choice"
    case .signature: value["field_type"] = "signature"
    default: value["field_type"] = "unknown(\(field_type.rawValue))"
    }
    switch annotation.widgetControlType.rawValue {
    case 1: value["button_type"] = "radio"
    case 2: value["button_type"] = "checkbox"
    case 0: value["button_type"] = "pushButton"
    default: value["button_type"] = "unknown"
    }
    switch annotation.buttonWidgetState.rawValue {
    case 1: value["button_state"] = "on"
    case 0: value["button_state"] = "off"
    case -1: value["button_state"] = "mixed"
    default: value["button_state"] = "unknown"
    }
    value["button_state_string"] = annotation.buttonWidgetStateString
    if field_type == .choice {
        value["is_list_choice"] = annotation.isListChoice
    }
    if let text = annotation.widgetStringValue { value["string_value"] = text }
    if let choices = annotation.choices { value["choices"] = choices }
    if let values = annotation.values { value["values"] = values.map { String(describing: $0) } }
    value["is_read_only"] = annotation.isReadOnly
    value["rect"] = [
        annotation.bounds.origin.x, annotation.bounds.origin.y,
        annotation.bounds.size.width, annotation.bounds.size.height,
    ]
    return value
}

func render(_ page: PDFPage, index: Int, outDir: URL) -> String? {
    let bounds = page.bounds(for: .cropBox)
    let scale: CGFloat = 2.0
    let size = NSSize(width: bounds.width * scale, height: bounds.height * scale)
    let thumbnail = page.thumbnail(of: size, for: .cropBox)
    guard let tiff = thumbnail.tiffRepresentation,
          let bitmap = NSBitmapImageRep(data: tiff),
          let png = bitmap.representation(using: .png, properties: [:]) else {
        return nil
    }
    let url = outDir.appendingPathComponent("pdfkit-page-\(index).png")
    do {
        try png.write(to: url)
        return url.path
    } catch {
        return nil
    }
}

let arguments = CommandLine.arguments
guard arguments.count >= 2 else {
    FileHandle.standardError.write("usage: pdfkit_probe.swift file.pdf [out-dir]\n".data(using: .utf8)!)
    exit(2)
}
let pdfURL = URL(fileURLWithPath: arguments[1])
let outDir = arguments.count >= 3
    ? URL(fileURLWithPath: arguments[2])
    : URL(fileURLWithPath: NSTemporaryDirectory())
try? FileManager.default.createDirectory(at: outDir, withIntermediateDirectories: true)

guard let document = PDFDocument(url: pdfURL) else {
    FileHandle.standardError.write("PDFKit could not open \(pdfURL.path)\n".data(using: .utf8)!)
    exit(1)
}

var pages: [[String: Any]] = []
for index in 0..<document.pageCount {
    guard let page = document.page(at: index) else { continue }
    let media = page.bounds(for: .mediaBox)
    let crop = page.bounds(for: .cropBox)
    let annotations = page.annotations.map(dictionary)
    var pageInfo: [String: Any] = [
        "index": index,
        "rotation": page.rotation,
        "media_box": [media.origin.x, media.origin.y, media.width, media.height],
        "crop_box": [crop.origin.x, crop.origin.y, crop.width, crop.height],
        "annotation_count": annotations.count,
        "annotations": annotations,
    ]
    if let png = render(page, index: index, outDir: outDir) {
        pageInfo["render"] = png
    }
    pages.append(pageInfo)
}

let report: [String: Any] = [
    "reader": "PDFKit",
    "path": pdfURL.path,
    "page_count": document.pageCount,
    "is_locked": document.isLocked,
    "allows_forms": true,
    "pages": pages,
]
let data = try JSONSerialization.data(withJSONObject: report, options: [.prettyPrinted, .sortedKeys])
print(String(data: data, encoding: .utf8)!)
