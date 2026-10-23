import type { Backend } from "./api";
import { MockBackend } from "./mockBackend";
import { tauriBackend } from "./tauriBackend";

export const inTauri = "__TAURI_INTERNALS__" in window;
export const backend: Backend = inTauri ? tauriBackend : new MockBackend();
