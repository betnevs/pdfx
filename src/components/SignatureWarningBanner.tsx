import type { SignatureSummary } from "../lib/backend";
import { SIGNATURE_WARNING_TITLE, signatureWarningText } from "../lib/document-policy";

export interface SignatureWarningBannerProps {
  /** Counts detected when the document was opened; never signer details. */
  signature: SignatureSummary;
  /** A save has already invalidated the actual signatures. */
  invalidated: boolean;
  onDismiss: () => void;
}

/**
 * Non-blocking notice shown while a document with signature structures is
 * open.
 *
 * It explains in Chinese that editing or saving invalidates signatures and
 * that PDFX never performs certificate signing. Dismissing hides it for the
 * session; a later successful save brings it back in the invalidated state.
 * The banner never renders signer identities, reasons or signature bytes.
 */
export function SignatureWarningBanner({
  signature,
  invalidated,
  onDismiss,
}: SignatureWarningBannerProps) {
  const copy = signatureWarningText(signature, invalidated);
  return (
    <div
      className={`signature-warning${invalidated ? " signature-warning--invalidated" : ""}`}
      role="status"
      aria-label={SIGNATURE_WARNING_TITLE}
    >
      <div className="signature-warning-body">
        <p className="signature-warning-title">{copy.title}</p>
        <p>{copy.message}</p>
      </div>
      <button type="button" className="secondary-button" onClick={onDismiss}>
        关闭
      </button>
    </div>
  );
}
