import { useEffect, useRef, useState } from "react";
import {
  documentOutline,
  extractText,
  type DocumentView,
  type OutlineEntry,
} from "../lib/backend";
import { onDocumentChanged } from "../lib/documentEvents";
import { WorkspaceIcon } from "./WorkspaceIcon";
export type NavigationTab = "thumbnails" | "outline" | "bookmarks" | "search";

/** Outline destinations come from the PDF. Reading bookmarks stay on this device. */
export function DocumentNavigation({
  document,
  tab,
  pageIndex,
  onSelectPage,
}: {
  document: DocumentView;
  tab: NavigationTab;
  pageIndex: number;
  onSelectPage: (page: number) => void;
}) {
  const [outline, setOutline] = useState<OutlineEntry[]>([]);
  const [error, setError] = useState("");
  const [revision, setRevision] = useState(0);
  const storageKey = `pdfx:reading-bookmarks:v1:${document.path}`;
  const [bookmarks, setBookmarks] = useState<number[]>(() => {
    try {
      const value: unknown = JSON.parse(
        localStorage.getItem(storageKey) ?? "[]",
      );
      return Array.isArray(value)
        ? value.filter(
            (n): n is number =>
              Number.isInteger(n) && n >= 0 && n < document.pageCount,
          )
        : [];
    } catch {
      return [];
    }
  });
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<{ page: number; snippet: string }[]>(
    [],
  );
  const [busy, setBusy] = useState(false);
  const request = useRef(0);
  useEffect(
    () => () => {
      request.current++;
    },
    [],
  );
  useEffect(
    () =>
      onDocumentChanged((change) => {
        if (change.documentId === document.documentId) {
          request.current++;
          setBusy(false);
          setResults([]);
          if (change.pageStructureChanged) {
            setRevision((r) => r + 1);
            setBookmarks([]);
            localStorage.removeItem(storageKey);
          }
        }
      }),
    [document.documentId, storageKey],
  );
  useEffect(() => {
    if (tab !== "outline") return;
    let cancelled = false;
    setError("");
    documentOutline(document.documentId)
      .then((items) => {
        if (!cancelled) setOutline(items);
      })
      .catch(() => {
        if (!cancelled) setError("无法读取文档大纲");
      });
    return () => {
      cancelled = true;
    };
  }, [document.documentId, tab, revision]);
  function saveBookmarks(next: number[]) {
    setBookmarks(next);
    try {
      localStorage.setItem(storageKey, JSON.stringify(next));
    } catch {
      setError("无法保存书签");
    }
  }
  async function search() {
    const token = ++request.current;
    const needle = query.trim().toLocaleLowerCase();
    setResults([]);
    setError("");
    if (!needle) {
      setBusy(false);
      return;
    }
    setBusy(true);
    for (let page = 0; page < document.pageCount; page++) {
      try {
        const value = await extractText(document.documentId, page);
        if (token !== request.current) return;
        const offset = value.text.toLocaleLowerCase().indexOf(needle);
        if (offset >= 0)
          setResults((rs) => [
            ...rs,
            {
              page,
              snippet: value.text.slice(
                Math.max(0, offset - 24),
                offset + needle.length + 64,
              ),
            },
          ]);
      } catch {
        if (token === request.current) setError("部分页面无法搜索");
      }
    }
    if (token === request.current) setBusy(false);
  }
  return (
    <div className="document-navigation-panel">
      {tab === "outline" && (
        <>
          <h2>大纲</h2>
          {outline.length === 0 && <p>{error || "此文档没有大纲"}</p>}
          {outline.map((item, i) => (
            <button
              type="button"
              key={i}
              className="navigation-row"
              style={{ paddingLeft: 12 + item.depth * 12 }}
              disabled={item.pageIndex === null}
              onClick={() =>
                item.pageIndex !== null && onSelectPage(item.pageIndex)
              }
            >
              {item.title}
            </button>
          ))}
        </>
      )}
      {tab === "bookmarks" && (
        <>
          <h2>书签</h2>
          <button
            type="button"
            className="navigation-add"
            onClick={() =>
              saveBookmarks(
                [...new Set([...bookmarks, pageIndex])].sort((a, b) => a - b),
              )
            }
          >
            <WorkspaceIcon name="plus" />
            添加当前页书签
          </button>
          {bookmarks.length === 0 && <p>尚无书签</p>}
          {bookmarks.map((page) => (
            <div className="bookmark-row" key={page}>
              <button type="button" onClick={() => onSelectPage(page)}>
                第 {page + 1} 页
              </button>
              <button
                type="button"
                aria-label={`删除第 ${page + 1} 页书签`}
                onClick={() =>
                  saveBookmarks(bookmarks.filter((p) => p !== page))
                }
              >
                <WorkspaceIcon name="trash" />
              </button>
            </div>
          ))}
        </>
      )}
      {tab === "search" && (
        <>
          <h2>搜索</h2>
          <form
            onSubmit={(e) => {
              e.preventDefault();
              void search();
            }}
          >
            <input
              autoFocus
              aria-label="搜索文档"
              placeholder="搜索文档"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
            />
            <button type="submit" aria-label="查找">
              <WorkspaceIcon name="search" />
            </button>
          </form>
          <p role="status">
            {busy
              ? `正在搜索… 已找到 ${results.length} 页`
              : `找到 ${results.length} 页`}
          </p>
          {results.map((result) => (
            <button
              type="button"
              className="search-result"
              key={result.page}
              onClick={() => onSelectPage(result.page)}
            >
              <strong>第 {result.page + 1} 页</strong>
              <span>{result.snippet}</span>
            </button>
          ))}
          {error && <p role="alert">{error}</p>}
        </>
      )}
    </div>
  );
}
