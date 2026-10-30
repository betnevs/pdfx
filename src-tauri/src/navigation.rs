//! Read the document outline through the same local PDFium session as the viewer.
use crate::commands::{with_open_document, CommandError, OpenDocument};
use pdfium_render::prelude::PdfBookmark;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutlineEntry {
    pub title: String,
    pub page_index: Option<u32>,
    pub depth: usize,
}

fn walk(
    bookmark: PdfBookmark<'_>,
    depth: usize,
    entry: &OpenDocument,
    result: &mut Vec<OutlineEntry>,
) {
    if depth > 32 || result.len() >= 10_000 {
        return;
    }
    result.push(OutlineEntry {
        title: bookmark.title().unwrap_or_else(|| "未命名".to_owned()),
        page_index: bookmark
            .destination()
            .and_then(|d| d.page_index().ok())
            .and_then(|p| u32::try_from(p).ok())
            .and_then(|p| entry.logical_page(p)),
        depth,
    });
    for child in bookmark.iter_direct_children().take(10_000) {
        walk(child, depth + 1, entry, result);
        if result.len() >= 10_000 {
            break;
        }
    }
}

#[tauri::command]
pub fn document_outline(document_id: String) -> Result<Vec<OutlineEntry>, CommandError> {
    with_open_document(&document_id, |entry| {
        let mut result = Vec::new();
        if let Some(root) = entry.document.bookmarks().root() {
            walk(root.clone(), 0, entry, &mut result);
            for bookmark in root.iter_siblings().take(10_000) {
                walk(bookmark, 0, entry, &mut result);
                if result.len() >= 10_000 {
                    break;
                }
            }
        }
        Ok(result)
    })
}
