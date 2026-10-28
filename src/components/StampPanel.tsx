import type { StampKind } from "../lib/backend";
import { STAMP_COLORS, STAMP_KINDS, STAMP_LABELS } from "../lib/stampCatalog";

export interface StampPanelProps {
  /** The stamp kind waiting to be placed, or null when none is armed. */
  armed: StampKind | null;
  /** PDF permission for adding annotations. */
  canAnnotate: boolean;
  /** Permission Restrictions deny every editing capability. */
  readOnly: boolean;
  onPick: (kind: StampKind | null) => void;
}

/**
 * Built-in stamp catalogue.
 *
 * Choosing a stamp arms it; the next click on the current page places it as a
 * native Stamp annotation. The previews use the same Chinese labels and ink
 * colours as the PDF appearance, so the panel reads as a stamp rack.
 */
export function StampPanel({
  armed,
  canAnnotate,
  readOnly,
  onPick,
}: StampPanelProps) {
  const editable = canAnnotate && !readOnly;
  return (
    <section className="stamp-panel" aria-label="图章面板">
      <header className="stamp-panel-header">
        <h2>图章</h2>
        <span>内置</span>
      </header>

      {!editable && (
        <p className="stamp-panel-hint">此文档不允许添加图章</p>
      )}

      <div className="stamp-grid">
        {STAMP_KINDS.map((kind) => {
          const label = STAMP_LABELS[kind];
          const isArmed = armed === kind;
          return (
            <button
              key={kind}
              type="button"
              className={`stamp-choice${isArmed ? " stamp-choice--armed" : ""}`}
              aria-label={`放置${label}图章`}
              aria-pressed={isArmed}
              disabled={!editable}
              onClick={() => onPick(isArmed ? null : kind)}
            >
              <span
                className="stamp-preview"
                aria-hidden="true"
                data-stamp-kind={kind}
                style={{ color: STAMP_COLORS[kind] }}
              >
                {label}
              </span>
            </button>
          );
        })}
      </div>

      {armed && editable && (
        <p className="stamp-placement-hint" role="status">
          在页面上单击放置“{STAMP_LABELS[armed]}”图章。
          <button
            type="button"
            className="stamp-cancel"
            onClick={() => onPick(null)}
          >
            取消
          </button>
        </p>
      )}
    </section>
  );
}
