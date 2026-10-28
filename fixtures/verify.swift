#!/usr/bin/env swift
// Independent fixture verification using macOS PDFKit and CryptoKit.
//
//     swift fixtures/verify.swift
//
// PDFKit is a different PDF implementation from the Python generator, so this
// script catches fixtures that merely "look" right in the generator's output:
// encrypted files must actually require a password, scanned pages must have no
// text layer, rotated pages must carry rotation, and so on.

import CryptoKit
import Foundation
import PDFKit

struct Fixture: Decodable {
    let id: String
    let category: String
    let path: String
    let sha256: String
    let sizeBytes: Int
    let pageCount: Int
    let valid: Bool
    let password: String?
    let ownerPassword: String?
}

struct Manifest: Decodable {
    let version: Int
    let fixtures: [Fixture]
}

let fixturesDir = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
var failures: [String] = []

func expect(_ condition: Bool, _ message: String) {
    if condition {
        print("  ok   \(message)")
    } else {
        failures.append(message)
        print("  FAIL \(message)")
    }
}

func containsASCII(_ data: Data, _ needle: String) -> Bool {
    data.range(of: Data(needle.utf8)) != nil
}

func pageText(_ document: PDFDocument, _ index: Int) -> String {
    document.page(at: index)?.string ?? ""
}

do {
    let manifestData = try Data(contentsOf: fixturesDir.appendingPathComponent("manifest.json"))
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    let manifest = try decoder.decode(Manifest.self, from: manifestData)

    print("verifying \(manifest.fixtures.count) fixtures with PDFKit")

    for fixture in manifest.fixtures {
        print("[\(fixture.category)] \(fixture.id)")

        let url = fixturesDir.appendingPathComponent(fixture.path)
        guard let bytes = try? Data(contentsOf: url) else {
            expect(false, "fixture file exists at \(fixture.path)")
            continue
        }

        expect(bytes.count == fixture.sizeBytes, "size matches manifest (\(bytes.count) bytes)")
        let digest = SHA256.hash(data: bytes).map { String(format: "%02x", $0) }.joined()
        expect(digest == fixture.sha256, "sha256 matches manifest")
        expect(bytes.starts(with: Data("%PDF-".utf8)), "starts with %PDF-")

        let document = PDFDocument(url: url)

        if fixture.category == "malformed" {
            if fixture.id == "malformed-truncated" {
                expect(document == nil, "PDFKit refuses the truncated file")
            } else {
                print("  info PDFKit open result: \(document == nil ? "refused" : "opened (repaired)")")
            }
            continue
        }

        guard let document else {
            expect(false, "PDFKit opens the file")
            continue
        }

        expect(document.pageCount == fixture.pageCount, "page count is \(fixture.pageCount)")

        switch fixture.category {
        case "text":
            let text = pageText(document, 0)
            expect(text.contains("PDFX fixture: text layer."), "text layer is extractable")
            expect(text.contains("extractable"), "second line survives extraction")

        case "scanned":
            expect(pageText(document, 0).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                   "scanned page has no text layer")
            expect(containsASCII(bytes, "/Subtype /Image"), "page uses an image XObject")

        case "encrypted":
            expect(document.isEncrypted, "document reports encryption")
            if let password = fixture.password, !password.isEmpty {
                expect(document.isLocked, "document is locked before the password is supplied")
                expect(document.unlock(withPassword: password), "unlocks with the documented user password")
            } else {
                _ = document.unlock(withPassword: "")
            }
            expect(!document.isLocked, "document is unlocked")
            expect(pageText(document, 0).contains("PDFX encrypted fixture."),
                   "page text is readable after unlocking")
            if fixture.id.contains("owner-restrictions") {
                expect(!document.allowsDocumentChanges, "owner restrictions deny document changes")
                expect(!document.allowsCopying, "owner restrictions deny copying")
                expect(document.allowsPrinting, "owner restrictions still allow printing")
            }
            if fixture.id.contains("print-denied") {
                expect(!document.allowsPrinting, "print restriction denies printing")
                expect(document.allowsDocumentChanges, "print-only restriction still allows changes")
                expect(document.allowsCopying, "print-only restriction still allows copying")
            }

        case "form":
            let pageZero = document.page(at: 0)?.annotations ?? []
            let pageOne = document.page(at: 1)?.annotations ?? []
            expect(pageZero.count == 8,
                   "page 1 has eight widget annotations (got \(pageZero.count))")
            expect(pageOne.count == 4,
                   "page 2 has four widget annotations (got \(pageOne.count))")
            expect((pageZero + pageOne).allSatisfy { ($0.type ?? "").contains("Widget") },
                   "all annotations are widgets")
            expect(containsASCII(bytes, "/AcroForm"), "catalog declares /AcroForm")
            expect(containsASCII(bytes, "/Off") && containsASCII(bytes, "/Yes"),
                   "checkbox exposes /Off and /Yes states")
            expect(containsASCII(bytes, "/male") && containsASCII(bytes, "/female"),
                   "radio group exposes export states")
            expect(containsASCII(bytes, "/Opt"), "choice fields declare options")
            let names = Set((pageZero + pageOne).compactMap { $0.fieldName })
            for name in ["full_name", "email", "subscribe", "gender", "country",
                         "department", "notes", "agree", "js_guard", "readonly_note"] {
                expect(names.contains(name), "form field \(name) is present")
            }

        case "structure":
            expect(document.outlineRoot != nil, "document has an outline")
            expect(document.outlineRoot?.numberOfChildren == 2,
                   "outline has two top-level items")
            let links = (document.page(at: 0)?.annotations ?? []).compactMap {
                ($0.action as? PDFActionURL)?.url?.absoluteString
            }
            expect(links.contains("https://example.com/pdfx"), "page has the URI link")
            expect(containsASCII(bytes, "/EmbeddedFile"), "file embeds an attachment")
            expect(containsASCII(bytes, "<x:xmpmeta"), "file contains XMP metadata")

        case "rotated":
            let rotations = (0..<document.pageCount).compactMap { document.page(at: $0)?.rotation }
            expect(rotations == [90, 0, 270], "page rotations are 90/0/270 (got \(rotations))")
            let sizes = (0..<document.pageCount).compactMap {
                document.page(at: $0)?.bounds(for: .mediaBox).size
            }
            expect(Set(sizes.map { "\(Int($0.width))x\(Int($0.height))" }).count == 3,
                   "three distinct page sizes")

        case "large":
            expect(fixture.pageCount >= 500, "large fixture has at least 500 pages")

        default:
            expect(false, "unknown fixture category \(fixture.category)")
        }
    }
} catch {
    print("verification error: \(error)")
    exit(2)
}

if failures.isEmpty {
    print("all fixture checks passed")
    exit(0)
} else {
    print("\(failures.count) fixture check(s) failed")
    exit(1)
}
