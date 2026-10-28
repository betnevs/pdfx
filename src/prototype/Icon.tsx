import type { ReactNode } from "react";

export type IconName =
  | "highlight"
  | "underline"
  | "strikeout"
  | "note"
  | "ink"
  | "rect"
  | "ellipse"
  | "line"
  | "arrow"
  | "image"
  | "signature"
  | "link"
  | "stamp"
  | "editText"
  | "addText"
  | "formText"
  | "formCheckbox"
  | "formRadio"
  | "formSelect"
  | "undo"
  | "redo"
  | "save"
  | "chevronLeft"
  | "chevronRight"
  | "chevronDown"
  | "zoomIn"
  | "zoomOut"
  | "fitWidth"
  | "fitPage"
  | "collapseLeft"
  | "collapseRight"
  | "expandLeft"
  | "expandRight"
  | "page"
  | "close"
  | "check"
  | "bold"
  | "italic"
  | "alignLeft"
  | "alignCenter"
  | "alignRight";

const ICON_PATHS: Record<IconName, ReactNode> = {
  highlight: (
    <>
      <path d="M15.2 4.6 19.4 8.8 10.6 17.6H6.4v-4.2z" />
      <path d="M4 21h16" />
    </>
  ),
  underline: (
    <>
      <path d="M7 4v6.4a5 5 0 0 0 10 0V4" />
      <path d="M5 21h14" />
    </>
  ),
  strikeout: (
    <>
      <path d="M6.5 12h11" />
      <path d="M9 6.4c.6-1.5 1.8-2.2 3.3-2.2 1.9 0 3.2 1 3.4 2.6" />
      <path d="M8.4 17.2c.5 1.4 1.8 2.1 3.8 2.1 2.1 0 3.5-1 3.5-2.6 0-.7-.3-1.3-.8-1.8" />
    </>
  ),
  note: (
    <>
      <path d="M5 4h14v11l-5 5H5z" />
      <path d="M14 20v-5h5" />
      <path d="M8.5 9h7M8.5 12.5h4" />
    </>
  ),
  ink: (
    <>
      <path d="M4 20.2c2.2-1 2.7-4.1 4.3-6.6 1.1-1.7 2.4-2.9 3.9-2.9 1.7 0 2.6 1.4 1.7 2.7-.8 1.1-2.2.9-2.9.1-1-1.2-.2-3 1.6-4.3 1.9-1.4 4.3-1.3 6.4.2" />
      <path d="M4 20.2h16" />
    </>
  ),
  rect: <rect x="4.5" y="5.5" width="15" height="13" rx="1.2" />,
  ellipse: <ellipse cx="12" cy="12" rx="8" ry="6.4" />,
  line: <path d="m5 19 14-14" />,
  arrow: (
    <>
      <path d="M5 19 19 5" />
      <path d="M12.5 5H19v6.5" />
    </>
  ),
  image: (
    <>
      <rect x="4" y="5" width="16" height="14" rx="1.6" />
      <circle cx="9" cy="10" r="1.6" />
      <path d="m5.5 17 4.2-4.2 3 3 2.4-2.4 3.4 3.4" />
    </>
  ),
  signature: (
    <>
      <path d="M4.5 16.5c2.3-.4 3.2-5.4 4.7-7.6 1-1.5 2.2-2.3 3.2-1.8 1.2.6.5 2.4-.5 3.6-1.3 1.6-2.8 2.4-3.2 3.4-.3.9.8 1.2 2.2.7 1.8-.6 3.1-1.6 4.1-1.4.9.2.6 1.3 1.2 1.6.7.4 1.7-.2 2.3-.7" />
      <path d="M5 20h14" />
    </>
  ),
  link: (
    <>
      <path d="M10 14.2a4 4 0 0 0 5.7 0l2.4-2.4a4 4 0 0 0-5.7-5.7l-1.2 1.2" />
      <path d="M14 9.8a4 4 0 0 0-5.7 0l-2.4 2.4a4 4 0 0 0 5.7 5.7l1.2-1.2" />
    </>
  ),
  stamp: (
    <>
      <rect x="4.5" y="14" width="15" height="5.5" rx="1.4" />
      <path d="M9 14v-2.2h6V14" />
      <path d="M12 3.5v3.2" />
      <path d="M9.5 6.7h5" />
    </>
  ),
  editText: (
    <>
      <path d="M5 6V4.5h14V6" />
      <path d="M12 4.5V19" />
      <path d="M8.5 19h7" />
    </>
  ),
  addText: (
    <>
      <path d="M4 6V4.5h11V6" />
      <path d="M9.5 4.5V16" />
      <path d="M7 16h5" />
      <path d="M17 13v6M14 16h6" />
    </>
  ),
  formText: (
    <>
      <rect x="4" y="5" width="16" height="14" rx="1.6" />
      <path d="M8.5 9.5h7M12 9.5V15M10 15h4" />
    </>
  ),
  formCheckbox: (
    <>
      <rect x="4" y="5" width="16" height="14" rx="1.6" />
      <rect x="7.5" y="9" width="6" height="6" rx="0.8" />
      <path d="m9.3 12 1.3 1.3 2.6-2.8" />
    </>
  ),
  formRadio: (
    <>
      <rect x="4" y="5" width="16" height="14" rx="1.6" />
      <circle cx="10.5" cy="12" r="3.2" />
      <circle cx="10.5" cy="12" r="1.2" />
      <path d="M16 10.5h1.5M16 13.5h1.5" />
    </>
  ),
  formSelect: (
    <>
      <rect x="4" y="5" width="16" height="14" rx="1.6" />
      <path d="M7.5 10.5h5" />
      <path d="m14.5 10.5 2 2 2-2" />
    </>
  ),
  undo: (
    <>
      <path d="M9 7 5 11l4 4" />
      <path d="M5 11h8.5a5.5 5.5 0 0 1 0 11H13" />
    </>
  ),
  redo: (
    <>
      <path d="m15 7 4 4-4 4" />
      <path d="M19 11h-8.5a5.5 5.5 0 0 0 0 11H11" />
    </>
  ),
  save: (
    <>
      <path d="M5 4h11l3 3v13H5z" />
      <path d="M8 4v5h7V4" />
      <path d="M8 20v-6h8v6" />
    </>
  ),
  chevronLeft: <path d="m14.5 6-6 6 6 6" />,
  chevronRight: <path d="m9.5 6 6 6-6 6" />,
  chevronDown: <path d="m6 9.5 6 6 6-6" />,
  zoomIn: (
    <>
      <circle cx="10.8" cy="10.8" r="6.2" />
      <path d="m15.4 15.4 4.1 4.1" />
      <path d="M8 10.8h5.6M10.8 8v5.6" />
    </>
  ),
  zoomOut: (
    <>
      <circle cx="10.8" cy="10.8" r="6.2" />
      <path d="m15.4 15.4 4.1 4.1" />
      <path d="M8 10.8h5.6" />
    </>
  ),
  fitWidth: (
    <>
      <path d="M7 5h10a1 1 0 0 1 1 1v12a1 1 0 0 1-1 1H7a1 1 0 0 1-1-1V6a1 1 0 0 1 1-1Z" />
      <path d="M3 12h3M18 12h3M4.5 10.5 3 12l1.5 1.5M19.5 10.5 21 12l-1.5 1.5" />
    </>
  ),
  fitPage: (
    <>
      <path d="M8 4h8a1.5 1.5 0 0 1 1.5 1.5v13A1.5 1.5 0 0 1 16 20H8a1.5 1.5 0 0 1-1.5-1.5v-13A1.5 1.5 0 0 1 8 4Z" />
      <path d="M4 8V4h4M20 8V4h-4M4 16v4h4M20 16v4h-4" />
    </>
  ),
  collapseLeft: (
    <>
      <path d="M14.5 6 8.5 12l6 6" />
      <path d="M5 5v14" />
    </>
  ),
  collapseRight: (
    <>
      <path d="m9.5 6 6 6-6 6" />
      <path d="M19 5v14" />
    </>
  ),
  expandLeft: (
    <>
      <path d="m9.5 6 6 6-6 6" />
      <path d="M19 5v14" />
    </>
  ),
  expandRight: (
    <>
      <path d="M14.5 6 8.5 12l6 6" />
      <path d="M5 5v14" />
    </>
  ),
  page: (
    <>
      <path d="M6 3.5h8l4 4V20.5H6z" />
      <path d="M14 3.5v4h4" />
    </>
  ),
  close: <path d="m6 6 12 12M18 6 6 18" />,
  check: <path d="m5 12.5 4.5 4.5L19 7" />,
  bold: (
    <>
      <path d="M7.5 4.5h5a3.7 3.7 0 0 1 0 7.4h-5z" />
      <path d="M7.5 11.9h5.7a3.8 3.8 0 0 1 0 7.6H7.5z" />
    </>
  ),
  italic: (
    <>
      <path d="M10 4.5h7.5M6.5 19.5H14" />
      <path d="M14.5 4.5 9.5 19.5" />
    </>
  ),
  alignLeft: (
    <>
      <path d="M4 6h16M4 10.5h10M4 15h16M4 19.5h10" />
    </>
  ),
  alignCenter: (
    <>
      <path d="M4 6h16M7 10.5h10M4 15h16M7 19.5h10" />
    </>
  ),
  alignRight: (
    <>
      <path d="M4 6h16M10 10.5h10M4 15h16M10 19.5h10" />
    </>
  ),
};

export function Icon({
  name,
  className,
}: {
  name: IconName;
  className?: string;
}) {
  return (
    <svg
      className={className ? `icon ${className}` : "icon"}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.7}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      focusable="false"
    >
      {ICON_PATHS[name]}
    </svg>
  );
}
