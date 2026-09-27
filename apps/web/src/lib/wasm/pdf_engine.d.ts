/* tslint:disable */
/* eslint-disable */
/**
 * Run a Tool entirely on-device and return its Output_File(s).
 *
 * `request_json` is a JSON string matching [`ToolRequest`]; `sources` is the
 * list of Source_File byte buffers (one `Uint8Array` per source, in the same
 * order as `source_names`).
 *
 * Returns a JS array of `{ name: string, bytes: Uint8Array }` on success.
 *
 * # Errors
 *
 * Throws a JS `Error` when the request JSON is malformed or when the engine
 * rejects the Job; the message is the mapped [`EngineError`] text.
 */
export function run_tool(request_json: string, sources: Uint8Array[]): any;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
  readonly memory: WebAssembly.Memory;
  readonly run_tool: (a: number, b: number, c: number, d: number) => [number, number, number];
  readonly __wbindgen_exn_store: (a: number) => void;
  readonly __externref_table_alloc: () => number;
  readonly __wbindgen_export_2: WebAssembly.Table;
  readonly __wbindgen_malloc: (a: number, b: number) => number;
  readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
  readonly __externref_table_dealloc: (a: number) => void;
  readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;
/**
* Instantiates the given `module`, which can either be bytes or
* a precompiled `WebAssembly.Module`.
*
* @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
*
* @returns {InitOutput}
*/
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
* If `module_or_path` is {RequestInfo} or {URL}, makes a request and
* for everything else, calls `WebAssembly.instantiate` directly.
*
* @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
*
* @returns {Promise<InitOutput>}
*/
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
