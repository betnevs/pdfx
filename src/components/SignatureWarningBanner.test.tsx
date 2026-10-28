import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import { SignatureWarningBanner } from "./SignatureWarningBanner";
import {
  SIGNATURE_FIELD_WARNING,
  SIGNATURE_INVALIDATED_WARNING,
  SIGNATURE_WARNING,
  SIGNATURE_WARNING_TITLE,
} from "../lib/document-policy";

describe("SignatureWarningBanner", () => {
  it("shows the Chinese invalidation warning for an actual signature", () => {
    render(
      <SignatureWarningBanner
        signature={{ signedCount: 1, fieldCount: 2 }}
        invalidated={false}
        onDismiss={() => {}}
      />,
    );

    const banner = screen.getByRole("status");
    expect(banner).toHaveTextContent(SIGNATURE_WARNING_TITLE);
    expect(banner).toHaveTextContent(SIGNATURE_WARNING);
    expect(banner).toHaveTextContent("编辑或保存会使签名失效");
    expect(banner).toHaveTextContent("PDFX 不执行证书签名");
    // Counts only: no signer identity, reason or signature bytes are rendered.
    expect(banner).not.toHaveTextContent("CANARY");
    expect(banner).not.toHaveTextContent("ByteRange");
  });

  it("uses the signature-field copy when no signature was ever applied", () => {
    render(
      <SignatureWarningBanner
        signature={{ signedCount: 0, fieldCount: 2 }}
        invalidated={false}
        onDismiss={() => {}}
      />,
    );

    expect(screen.getByRole("status")).toHaveTextContent(
      SIGNATURE_FIELD_WARNING,
    );
  });

  it("switches to the invalidated copy after a save", () => {
    render(
      <SignatureWarningBanner
        signature={{ signedCount: 1, fieldCount: 2 }}
        invalidated
        onDismiss={() => {}}
      />,
    );

    expect(screen.getByRole("status")).toHaveTextContent(
      SIGNATURE_INVALIDATED_WARNING,
    );
    expect(screen.getByRole("status")).toHaveTextContent("已失效");
  });

  it("can be dismissed without blocking the document", async () => {
    const onDismiss = vi.fn();
    const user = userEvent.setup();
    render(
      <SignatureWarningBanner
        signature={{ signedCount: 1, fieldCount: 2 }}
        invalidated={false}
        onDismiss={onDismiss}
      />,
    );

    await user.click(screen.getByRole("button", { name: "关闭" }));

    expect(onDismiss).toHaveBeenCalledTimes(1);
  });
});
