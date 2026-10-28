import { cleanup } from "@testing-library/react";
import { afterEach } from "vitest";

import "@testing-library/jest-dom/vitest";

// Vitest runs with explicit imports (no globals), so Testing Library cannot
// register its own automatic cleanup hook. Do it here for every test file.
afterEach(() => {
  cleanup();
});
