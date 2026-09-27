// Feature: pdf-tools-suite (Task 11.3, Req 48.3, 48.5)
//
// A tiny jobs store tracking every Job started in this browser session so the
// UI can show each running Job's status independently (Req 48.5) and warn on
// tab close while a Server_Side Job is still running (Req 48.3).

import { writable, derived, get, type Readable } from 'svelte/store';
import type { ProcessingMode, ToolId } from '../tools/registry';
import type { OutputFile } from '../engine/types';

export type JobPhase = 'queued' | 'running' | 'succeeded' | 'failed';

export interface JobRecord {
	id: string;
	toolId: ToolId;
	toolLabel: string;
	mode: ProcessingMode;
	phase: JobPhase;
	/** 0-100 progress when known; undefined for indeterminate work. */
	progress?: number;
	/** Failure reason surfaced to the User (Req 33.3). */
	error?: string;
	/** Original Source_File name(s) that fed this Job, for the panel's summary. */
	sourceNames?: string[];
	/** Produced Output_Files, set on success so the panel can offer downloads. */
	outputs?: OutputFile[];
	startedAt: number;
}

const store = writable<JobRecord[]>([]);

/** All Jobs in this session, most recent first. */
export const jobs: Readable<JobRecord[]> = { subscribe: store.subscribe };

/** Whether any Server_Side Job is still running (drives the beforeunload warning, Req 48.3). */
export const hasRunningServerJob: Readable<boolean> = derived(store, ($jobs) =>
	$jobs.some((j) => j.mode === 'Server_Side' && (j.phase === 'queued' || j.phase === 'running'))
);

let seq = 0;
function nextId(): string {
	seq += 1;
	return `job-${Date.now().toString(36)}-${seq}`;
}

/** Register a new Job and return its id. */
export function startJob(input: {
	toolId: ToolId;
	toolLabel: string;
	mode: ProcessingMode;
	phase?: JobPhase;
	progress?: number;
	sourceNames?: string[];
}): string {
	const id = nextId();
	const record: JobRecord = {
		id,
		toolId: input.toolId,
		toolLabel: input.toolLabel,
		mode: input.mode,
		phase: input.phase ?? 'running',
		progress: input.progress,
		sourceNames: input.sourceNames,
		startedAt: Date.now()
	};
	store.update((list) => [record, ...list]);
	return id;
}

/** Patch a Job record by id. */
export function updateJob(id: string, patch: Partial<Omit<JobRecord, 'id'>>): void {
	store.update((list) => list.map((j) => (j.id === id ? { ...j, ...patch } : j)));
}

/** Attach the produced Output_Files to a Job so the panel can offer downloads. */
export function setJobOutputs(id: string, outputs: OutputFile[]): void {
	updateJob(id, { outputs });
}

/** Remove a Job record (e.g. when the User dismisses it). */
export function removeJob(id: string): void {
	store.update((list) => list.filter((j) => j.id !== id));
}

/** Current snapshot (non-reactive) — handy in event handlers. */
export function currentJobs(): JobRecord[] {
	return get(store);
}
