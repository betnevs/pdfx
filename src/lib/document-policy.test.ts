import { describe, expect, it } from "vitest";

import type { DocumentPermissions, DocumentView } from "./backend";
import {
  documentPolicy,
  PRINT_DISABLED_REASON,
  READ_ONLY_REASON,
  SAVE_CHANGES_READ_ONLY,
  SIGNATURE_FIELD_WARNING,
  SIGNATURE_INVALIDATED_WARNING,
  SIGNATURE_WARNING,
  SIGNATURE_WARNING_TITLE,
  signatureWarningText,
  UNENCRYPTED_OUTPUT_WARNING,
} from "./document-policy";

const allGranted: DocumentPermissions = {
  canModifyContent: true,
  canAnnotate: true,
  canAssemble: true,
  canFillForms: true,
  canPrint: true,
  canCopyContent: true,
};

const denied: DocumentPermissions = {
  canModifyContent: false,
  canAnnotate: false,
  canAssemble: false,
  canFillForms: false,
  canPrint: true,
  canCopyContent: false,
};

function documentView(overrides: Partial<DocumentView> = {}): DocumentView {
  return {
    documentId: "doc-1",
    name: "report.pdf",
    path: "/tmp/report.pdf",
    pageCount: 1,
    pageWidth: 612,
    pageHeight: 792,
    encrypted: false,
    readOnly: false,
    permissions: allGranted,
    ...overrides,
  };
}

describe("document policy", () => {
  it("leaves an editable, unencrypted document unblocked", () => {
    expect(documentPolicy(documentView())).toEqual({
      readOnly: false,
      editingDisabledReason: null,
      saveChangesDisabledReason: null,
      unencryptedOutputWarning: null,
      printDisabledReason: null,
    });
  });

  it("explains why printing is unavailable when the owner denied it", () => {
    const policy = documentPolicy(
      documentView({ permissions: { ...allGranted, canPrint: false } }),
    );

    expect(policy.readOnly).toBe(false);
    expect(policy.printDisabledReason).toBe(PRINT_DISABLED_REASON);
    expect(policy.printDisabledReason).toContain("打印");
  });

  it("warns that saving an encrypted document produces an unencrypted copy", () => {
    const policy = documentPolicy(documentView({ encrypted: true }));

    expect(policy.readOnly).toBe(false);
    expect(policy.saveChangesDisabledReason).toBeNull();
    expect(policy.unencryptedOutputWarning).toBe(UNENCRYPTED_OUTPUT_WARNING);
    expect(policy.unencryptedOutputWarning).toContain("不加密");
  });

  it("blocks editing and Save Changes on a read-only document with a visible reason", () => {
    const policy = documentPolicy(
      documentView({ encrypted: true, readOnly: true, permissions: denied }),
    );

    expect(policy.readOnly).toBe(true);
    expect(policy.editingDisabledReason).toBe(READ_ONLY_REASON);
    expect(policy.saveChangesDisabledReason).toBe(SAVE_CHANGES_READ_ONLY);
    expect(policy.unencryptedOutputWarning).toBe(UNENCRYPTED_OUTPUT_WARNING);
  });

  it("trusts the backend read-only verdict instead of re-deriving it from flags", () => {
    // A document may deny annotations while still allowing content edits; the
    // backend decides the document-level state and individual tools consult
    // the permission flags.
    const policy = documentPolicy(
      documentView({
        readOnly: false,
        permissions: { ...allGranted, canAnnotate: false },
      }),
    );

    expect(policy.readOnly).toBe(false);
    expect(policy.editingDisabledReason).toBeNull();
    expect(policy.saveChangesDisabledReason).toBeNull();
  });
});

describe("signature warning copy", () => {
  it("warns in Chinese that editing or saving invalidates actual signatures", () => {
    const copy = signatureWarningText(
      { signedCount: 1, fieldCount: 2 },
      false,
    );

    expect(copy.title).toBe(SIGNATURE_WARNING_TITLE);
    expect(copy.message).toBe(SIGNATURE_WARNING);
    expect(copy.message).toContain("编辑或保存会使签名失效");
    expect(copy.message).toContain("PDFX 不执行证书签名");
  });

  it("distinguishes empty signature fields from actual signatures", () => {
    const copy = signatureWarningText({ signedCount: 0, fieldCount: 2 }, false);

    expect(copy.message).toBe(SIGNATURE_FIELD_WARNING);
    expect(copy.message).toContain("签名字段");
    expect(copy.message).not.toContain("会使签名失效");
  });

  it("states after a save that the signatures are now invalid", () => {
    const copy = signatureWarningText({ signedCount: 1, fieldCount: 2 }, true);

    expect(copy.message).toBe(SIGNATURE_INVALIDATED_WARNING);
    expect(copy.message).toContain("已失效");
    expect(copy.message).toContain("PDFX 不执行证书签名");
  });
});
