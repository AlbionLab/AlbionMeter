import type { DungeonSummary, Encounter } from './types';

export const isRecord = (encounter: Encounter) => !encounter.location;

export interface RecordWindow { start: number; end: number; runId?: string }

export function recordWindows(records: Encounter[]): Map<string, RecordWindow> {
  const sessions = records.filter(record => !record.location).sort((a, b) => Date.parse(a.startedAt) - Date.parse(b.startedAt));
  const windows = new Map<string, RecordWindow>(sessions.map((record, index) => [record.id, {
    start: Date.parse(record.startedAt), end: index + 1 < sessions.length ? Date.parse(sessions[index + 1].startedAt) : Infinity
  }]));
  for (const record of records) if (record.location) windows.set(record.id, { start: 0, end: 0, runId: record.location.runId });
  return windows;
}

export function inRecord(run: Pick<DungeonSummary, 'id' | 'startedAt' | 'mode'>, window: RecordWindow) {
  if (window.runId) return run.id === window.runId;
  const started = Date.parse(run.startedAt);
  return started >= window.start - 1000 && started < window.end;
}

export function recordOf(run: Pick<DungeonSummary, 'id' | 'startedAt' | 'mode'>, windows: Map<string, RecordWindow>) {
  return [...windows].find(([, window]) => inRecord(run, window))?.[0];
}
