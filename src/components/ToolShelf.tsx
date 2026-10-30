import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { createPortal } from "react-dom";

import { flushPendingEdits, hasPendingEdits } from "../lib/pendingEdits";
import { WorkspaceIcon, type WorkspaceIconName } from "./WorkspaceIcon";

/**
 * Where a page tool appears. The comment group is the reference editor's
 * quick annotation bar; the other five groups mirror its edit-mode tabs.
 */
export type ToolGroup =
  | "comment"
  | "annotate"
  | "shapes"
  | "insert"
  | "text"
  | "forms";

export interface ToolShelfTool {
  id: string;
  label: string;
  icon: WorkspaceIconName;
  /** Tabs and quick bars that show this tool. A tool may appear in several. */
  groups: readonly ToolGroup[];
  /** Whether the underlying layer mode is currently armed. */
  active?: boolean;
  disabled?: boolean;
  /** Tooltip; disabled tools explain why they cannot be used. */
  title?: string;
  /** Left-to-right position inside a row. */
  order?: number;
  /**
   * Toggles the tool. Omit for an action chip that only opens the shelf panel
   * (for example a style panel).
   */
  onActivate?: (next: boolean) => void;
}

interface EditorToolsContextValue {
  group: ToolGroup;
  showProperties: boolean;
  toggleProperties: () => void;
  chipsHost: HTMLDivElement | null;
  panelHost: HTMLDivElement | null;
  activeKey: string | null;
  setChipsHost: (node: HTMLDivElement | null) => void;
  setPanelHost: (node: HTMLDivElement | null) => void;
  activate: (key: string, deactivate: (() => void) | null) => void;
  release: (key: string) => void;
  /** Deactivates whatever tool is armed, without changing tabs. */
  closeActive: () => void;
}

const EditorToolsContext = createContext<EditorToolsContextValue | null>(null);

/** Chrome-owned controls use this to disarm the current page tool. */
export function useEditorTools(): EditorToolsContextValue | null {
  return useContext(EditorToolsContext);
}

/**
 * Owns the toolbar placement for every page layer. Chips are rendered where
 * `ToolChips` is mounted (the comment bar or the edit-mode icon rail) and the
 * armed tool's properties are rendered in `ToolPanelHost` (the inspector).
 *
 * Only one tool is armed at a time. Switching tabs or arming a different tool
 * calls the previous tool's off-toggle, so no invisible mode stays active.
 */
export function EditorToolsProvider({
  group,
  onToolActive,
  children,
}: {
  group: ToolGroup;
  /** Called whenever a tool is armed, so chrome modes like pan can stop. */
  onToolActive?: () => void;
  children: ReactNode;
}) {
  const [showProperties, setShowProperties] = useState(group === "text");
  const [chipsHost, setChipsHost] = useState<HTMLDivElement | null>(null);
  const [panelHost, setPanelHost] = useState<HTMLDivElement | null>(null);
  const [activeKey, setActiveKey] = useState<string | null>(null);
  const activeRef = useRef<string | null>(null);
  const groupRef = useRef(group);
  const deactivators = useRef(new Map<string, () => void>());
  const onToolActiveRef = useRef(onToolActive);
  onToolActiveRef.current = onToolActive;

  const release = useCallback((key: string) => {
    deactivators.current.delete(key);
    if (activeRef.current === key) {
      activeRef.current = null;
      setActiveKey(null);
    }
  }, []);

  const activate = useCallback(
    (key: string, deactivate: (() => void) | null) => {
      if (deactivate) {
        deactivators.current.set(key, deactivate);
      } else {
        deactivators.current.delete(key);
      }
      const previous = activeRef.current;
      if (previous && previous !== key) {
        const off = deactivators.current.get(previous);
        deactivators.current.delete(previous);
        off?.();
      }
      onToolActiveRef.current?.();
      activeRef.current = key;
      setActiveKey(key);
    },
    [],
  );

  const closeActive = useCallback(() => {
    const previous = activeRef.current;
    if (previous === null) {
      return;
    }
    const off = deactivators.current.get(previous);
    deactivators.current.delete(previous);
    activeRef.current = null;
    setActiveKey(null);
    off?.();
  }, []);

  // A tab switch must not leave a tool armed on a page the user can no longer
  // see controls for.
  useLayoutEffect(() => {
    if (groupRef.current === group) {
      return;
    }
    groupRef.current = group;
    closeActive();
    setShowProperties(group === "text" || group === "forms");
  }, [group, closeActive]);

  return (
    <EditorToolsContext.Provider
      value={{
        group,
        showProperties,
        toggleProperties: () => setShowProperties(value => !value),
        chipsHost,
        panelHost,
        activeKey,
        setChipsHost,
        setPanelHost,
        activate,
        release,
        closeActive,
      }}
    >
      {children}
    </EditorToolsContext.Provider>
  );
}

/** Mount point for the current tool's chips. */
export function ToolChips({
  className,
  label = "页面工具",
}: {
  className?: string;
  label?: string;
}) {
  const tools = useContext(EditorToolsContext);
  const setChipsHost = tools?.setChipsHost;
  const [host, setHost] = useState<HTMLDivElement | null>(null);

  useLayoutEffect(() => {
    if (!setChipsHost) {
      return;
    }
    setChipsHost(host);
    return () => setChipsHost(null);
  }, [host, setChipsHost]);

  return (
    <div
      ref={setHost}
      className={className ? `tool-chips ${className}` : "tool-chips"}
      role="toolbar"
      aria-label={label}
    />
  );
}

/** Mount point for the armed tool's properties, in the inspector. */
export function ToolPanelHost() {
  const tools = useContext(EditorToolsContext);
  const setPanelHost = tools?.setPanelHost;
  const [host, setHost] = useState<HTMLDivElement | null>(null);

  useLayoutEffect(() => {
    if (!setPanelHost) {
      return;
    }
    setPanelHost(host);
    return () => setPanelHost(null);
  }, [host, setPanelHost]);

  return <div ref={setHost} className="editor-tool-panel" data-properties-visible={tools?.showProperties ?? true} aria-label="工具属性" />;
}

/**
 * An action that behaves like a rail tool but is owned by the editor chrome
 * rather than a page layer (for example opening the stamp panel).
 */
export function ToolActionChip({
  group,
  label,
  icon,
  onClick,
  disabled = false,
  title,
  order,
}: {
  group: ToolGroup;
  label: string;
  icon: WorkspaceIconName;
  onClick: () => void;
  disabled?: boolean;
  title?: string;
  order?: number;
}) {
  const tools = useContext(EditorToolsContext);
  if (!tools || tools.group !== group) {
    return null;
  }
  const button = (
    <button
      type="button"
      className="tool-shelf-chip"
      aria-label={label}
      title={title ?? label}
      disabled={disabled}
      style={{ order: order ?? 100 }}
      onClick={() => {
        tools.closeActive();
        onClick();
      }}
      onPointerDown={(event) => event.stopPropagation()}
    >
      <WorkspaceIcon name={icon} className="tool-shelf-chip-icon" />
      <span className="tool-shelf-chip-label">{label}</span>
    </button>
  );
  return tools.chipsHost ? createPortal(button, tools.chipsHost) : null;
}

/**
 * One page layer's tools. Without `EditorToolsProvider` (layer unit tests) the
 * chips and the panel render inline next to the layer; inside the editor the
 * chips are portalled into the active row and the panel into the inspector.
 */
export function ToolShelf({
  tools,
  children,
  panelTitle = "工具属性",
  panelOpen: panelOpenOverride,
  defaultToolId,
}: {
  tools: readonly ToolShelfTool[];
  defaultToolId?: string;
  children?: ReactNode;
  /** Heading shown above the properties when the panel is hosted. */
  panelTitle?: string;
  /** Opens the panel without an armed tool, e.g. for a selected object. */
  panelOpen?: boolean;
}) {
  const editor = useContext(EditorToolsContext);
  const shelfKey = useId();
  const [localPanelId, setLocalPanelId] = useState<string | null>(null);
  const keyFor = (toolId: string) => `${shelfKey}:${toolId}`;
  // Deactivation happens later, often after a page or tool state update.
  // Read the latest callback instead of the closure from the activation render.
  const latestTools = useRef(tools);
  latestTools.current = tools;
  const changeTool = (id: string, next: boolean) => {
    const tool = latestTools.current.find(candidate => candidate.id === id);
    if (tool && tool.active !== next) tool.onActivate?.(next);
  };


  const ownedToolId = editor?.activeKey?.startsWith(`${shelfKey}:`)
    ? editor.activeKey.slice(shelfKey.length + 1)
    : null;
  const ownedTool = ownedToolId
    ? (tools.find((tool) => tool.id === ownedToolId) ?? null)
    : null;
  const groupMatches =
    !editor || tools.some((tool) => tool.groups.includes(editor.group));
  const ownedInGroup =
    ownedTool !== null &&
    (!editor || ownedTool.groups.includes(editor.group));

  // A tool deactivated from inside its layer (Escape, a completed placement,
  // a permission refusal) must release the editor-wide active key too.
  useEffect(() => {
    if (!editor || !ownedTool) {
      return;
    }
    if (ownedTool.onActivate && !ownedTool.active) {
      editor.release(keyFor(ownedTool.id));
    }
    // keyFor is derived from a stable useId.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editor, ownedTool]);

  const enteredGroup = useRef<string | null>(null);
  useEffect(() => {
    if (!editor) return;
    const group = editor.group;
    if (enteredGroup.current === group) return;
    const initial = tools.find(tool => tool.id === defaultToolId && tool.groups.includes(group));
    if (initial?.disabled) return;
    enteredGroup.current = group;
    if (initial?.onActivate) {
      if (!initial.active) changeTool(initial.id, true);
      editor.activate(keyFor(initial.id), () => changeTool(initial.id, false));
    }
  }, [editor, tools, defaultToolId]);

  const standaloneActive = tools.some((tool) => tool.active);
  const panelOpen = editor
    ? ownedTool !== null
      ? ownedInGroup
      : editor.activeKey === null && groupMatches && (panelOpenOverride ?? false)
    : standaloneActive || localPanelId !== null || (panelOpenOverride ?? false);

  function isActive(tool: ToolShelfTool): boolean {
    if (tool.onActivate) {
      return tool.active ?? false;
    }
    return editor
      ? editor.activeKey === keyFor(tool.id)
      : localPanelId === tool.id;
  }

  async function handleClick(tool: ToolShelfTool) {
    if (hasPendingEdits() && !(await flushPendingEdits())) return;
    if (tool.disabled) {
      return;
    }
    if (tool.onActivate) {
      const next = !tool.active;
      if (editor) {
        if (next) {
          editor.activate(keyFor(tool.id), typeof tool.active === "boolean" ? () => changeTool(tool.id, false) : null);
        } else {
          editor.release(keyFor(tool.id));
        }
      } else if (next) {
        setLocalPanelId(tool.id);
      } else {
        setLocalPanelId(null);
      }
      changeTool(tool.id, next);
      return;
    }
    if (editor) {
      if (editor.activeKey === keyFor(tool.id)) {
        editor.release(keyFor(tool.id));
      } else {
        editor.activate(keyFor(tool.id), null);
      }
      return;
    }
    setLocalPanelId((current) => (current === tool.id ? null : tool.id));
  }

  const visibleTools = editor
    ? tools.filter((tool) => tool.groups.includes(editor.group))
    : tools;

  const chips = visibleTools.map((tool) => {
    const active = isActive(tool);
    return (
      <button
        key={tool.id}
        type="button"
        className="tool-shelf-chip"
        data-tool={tool.id}
        data-active={active ? "true" : undefined}
        aria-label={tool.label}
        aria-pressed={tool.onActivate ? active : undefined}
        aria-expanded={
          tool.onActivate
            ? undefined
            : editor
              ? editor.activeKey === keyFor(tool.id)
              : localPanelId === tool.id
        }
        aria-controls={panelOpen ? `tool-shelf-panel-${shelfKey}` : undefined}
        disabled={tool.disabled}
        title={tool.title ?? tool.label}
        style={{ order: tool.order ?? 100 }}
        onClick={() => handleClick(tool)}
        onPointerDown={(event) => event.stopPropagation()}
      >
        <WorkspaceIcon name={tool.icon} className="tool-shelf-chip-icon" />
        <span className="tool-shelf-chip-label">{tool.label}</span>
      </button>
    );
  });

  const panel = (
    <div
      id={`tool-shelf-panel-${shelfKey}`}
      className="tool-shelf-panel"
      data-open={panelOpen ? "true" : "false"}
      data-title={panelTitle}
      onPointerDown={(event) => event.stopPropagation()}
    >
      {editor && <h2 className="tool-panel-title">{panelTitle}</h2>}
      {children}
    </div>
  );

  if (!editor) {
    return (
      <>
        <div className="tool-shelf-chips" role="toolbar" aria-label="页面工具">
          {chips}
        </div>
        {panel}
      </>
    );
  }

  return (
    <>
      {editor.chipsHost ? createPortal(chips, editor.chipsHost) : null}
      {panelOpen && editor.panelHost
        ? createPortal(panel, editor.panelHost)
        : null}
    </>
  );
}
