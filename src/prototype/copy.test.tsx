import { readFileSync, readdirSync } from "node:fs";
import { join, relative } from "node:path";

import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { PrototypeApp } from "./PrototypeApp";

// Vitest runs from the repository root (see vite.config.ts).
const PROTOTYPE_DIR = join(process.cwd(), "src", "prototype");

/** Files allowed to contain Simplified Chinese source text. */
const TRANSLATION_FILES = new Set(
  ["copy.ts", "content.ts"].map((name) => join(PROTOTYPE_DIR, name)),
);

function sourceFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const fullPath = join(dir, entry.name);
    if (entry.isDirectory()) {
      return sourceFiles(fullPath);
    }
    if (!/\.(ts|tsx)$/.test(entry.name) || /\.test\./.test(entry.name)) {
      return [];
    }
    return [fullPath];
  });
}

describe("prototype localisation", () => {
  it("keeps every Chinese string in the copy or placeholder content modules", () => {
    const offenders = sourceFiles(PROTOTYPE_DIR)
      .filter((file) => !TRANSLATION_FILES.has(file))
      .flatMap((file) => {
        const matches = readFileSync(file, "utf8").match(/[一-鿿]+/g);
        return matches
          ? [`${relative(PROTOTYPE_DIR, file)}: ${matches.join(", ")}`]
          : [];
      });

    expect(offenders).toEqual([]);
  });

  it("does not use iLovePDF branding in the prototype source or UI", () => {
    const branded = sourceFiles(PROTOTYPE_DIR).filter((file) =>
      /ilovepdf/i.test(readFileSync(file, "utf8")),
    );
    expect(branded).toEqual([]);

    render(<PrototypeApp />);
    expect(document.body.textContent ?? "").not.toMatch(/ilovepdf/i);
    expect(screen.getByRole("heading", { level: 1, name: "PDFX" })).toBeInTheDocument();
  });
});
