import { invoke } from '@tauri-apps/api/core';

/**
 * Personal to-dos (Todoist-like). Stored in the app data folder (`todos/todos.json`), never in a
 * project. Mirrors `devall_core::todos` (model + store); all edits go through `todosApi.apply`.
 */

/** 1 = p1 (most urgent) … 4 = p4 (no priority, the default). */
export type Priority = 1 | 2 | 3 | 4;
export type Day = 'mon' | 'tue' | 'wed' | 'thu' | 'fri' | 'sat' | 'sun';

export interface Recurrence {
  freq: 'day' | 'week' | 'month' | 'year';
  interval?: number;
  weekdays?: Day[];
  /** `-1` = last day of the month. */
  monthDays?: number[];
  weekOfMonth?: number;
  month?: number;
  /** Todoist's `every!`: counts from the completion date. */
  fromCompletion?: boolean;
  until?: string;
}

export interface Due {
  /** `YYYY-MM-DD` (wall clock in `timezone`, or local time when floating). */
  date: string;
  /** `HH:MM`. */
  time?: string;
  /** IANA zone for a fixed-time task. */
  timezone?: string;
  recurrence?: Recurrence;
  /** The rule as typed (`every mon 9am`), recurring dates only. */
  string?: string;
}

export interface Comment {
  id: string;
  text: string;
  at: number;
}

/** A board card's cover: one of the task's image attachments, or a colour (palette name or `#hex`). */
export interface Cover {
  attachmentId?: string;
  color?: string;
}

/** A file attached to a task: a copy in `<app data>/todos/attachments/<task id>/<name>`. */
export interface Attachment {
  id: string;
  /** Stored file name (sanitized, unique within the task). */
  name: string;
  mime: string;
  size: number;
  addedAt: number;
}

export interface Task {
  id: string;
  content: string;
  description?: string;
  listId: string;
  sectionId?: string;
  parentId?: string;
  order: number;
  priority: Priority;
  due?: Due;
  /** `YYYY-MM-DD`. */
  deadline?: string;
  labels?: string[];
  /** `http/…req.yaml`, `notes/…md`, `mocks/…`, URLs. */
  links?: string[];
  done?: boolean;
  completedAt?: number;
  createdAt: number;
  updatedAt: number;
  comments?: Comment[];
  attachments?: Attachment[];
  /** Completions of a recurring task. */
  history?: { at: number; due: string }[];
  collapsed?: boolean;
  /**
   * Board column (status) id. Missing or unknown = the first column; completed tasks are always in
   * the done column (for them this is where they go back to when reopened).
   */
  status?: string;
  /** Position in its board column (manual order across lists). */
  statusOrder?: number;
  /** Board card cover. */
  cover?: Cover;
}

export interface TaskView extends Task {
  /** Local calendar day it's due (differs from `due.date` only for fixed time zones). */
  dueLocal?: string;
  /** Due instant (ms) when it has a time. */
  dueAt?: number;
}

export type ListKind = 'inbox' | 'project' | 'user';

export interface TaskList {
  id: string;
  name: string;
  kind: ListKind;
  color?: string;
  order: number;
  projectRoot?: string;
}

export interface Section {
  id: string;
  listId: string;
  name: string;
  order: number;
  collapsed?: boolean;
}

export interface Label {
  name: string;
  /** Palette name or `#rrggbb`; missing = the name's default colour (`defaultLabelColor`). */
  color?: string;
}

export interface SavedFilter {
  id: string;
  name: string;
  query: string;
  color?: string;
  order: number;
}

export interface FilterGroup {
  title: string;
  ids: string[];
}

export interface FilterView extends SavedFilter {
  count: number;
  groups: FilterGroup[];
  error?: string;
}

/** A board column: one workflow status. */
export interface StatusColumn {
  /** Stable id (empty for a column being added: Rust assigns one). */
  id: string;
  name: string;
  /** Palette name. */
  color?: string | null;
  /** Work-in-progress limit (open cards), shown as `n/limit`. */
  wipLimit?: number | null;
  /** The done column (exactly one): moving a card into it completes the task. */
  done?: boolean;
}

export interface TodoSettings {
  reminders: boolean;
  remindBeforeMinutes: number;
  /** `HH:MM` reminder for dated tasks without a time. */
  allDayReminder?: string;
  /** Board columns in order; changed only with `saveColumns` (`saveSettings` ignores them). */
  columns: StatusColumn[];
}

export interface Snapshot {
  lists: TaskList[];
  sections: Section[];
  labels: Label[];
  filters: FilterView[];
  settings: TodoSettings;
  /** Open tasks, completed ones of the last 7 days and done sub-tasks of open tasks — every completed task when loaded with `allDone`. */
  tasks: TaskView[];
  /** The open project's list id. */
  projectList?: string | null;
  completedCount: number;
  /** Completed tasks left out of `tasks` (older than a week), per list id; empty with `allDone`. */
  olderDone: Record<string, number>;
  today: string;
}

/** A task to create: `text` is quick-add text unless `parse` is false. */
export interface NewTask {
  text: string;
  parse?: boolean;
  ignore?: string[];
  description?: string;
  listId?: string;
  sectionId?: string;
  parentId?: string;
  afterId?: string;
  priority?: Priority;
  due?: Due;
  dueString?: string;
  deadline?: string;
  labels?: string[];
  links?: string[];
  /** Board column id (the done column = the first column). */
  status?: string;
  /** First among its siblings and at the top of its board column. */
  atTop?: boolean;
}

export interface TaskPatch {
  content?: string;
  description?: string;
  priority?: Priority;
  /** `null` clears. */
  due?: Due | null;
  /** Natural language; `''` clears. */
  dueString?: string;
  /** `YYYY-MM-DD`: moves the date, keeping time and recurrence; `''` clears. */
  dueDate?: string;
  deadline?: string | null;
  labels?: string[];
  addLabels?: string[];
  removeLabels?: string[];
  links?: string[];
  addLinks?: string[];
  collapsed?: boolean;
  /** Board column id or name (the done column completes, leaving it reopens). */
  status?: string;
  /** An image attachment of the task or a colour; `null` removes the cover. */
  cover?: Cover | null;
}

export type TodoOp =
  | { type: 'add'; task: NewTask }
  | { type: 'edit'; id: string; text: string; ignore?: string[] }
  | { type: 'update'; ids: string[]; patch: TaskPatch }
  | { type: 'complete'; ids: string[]; done: boolean }
  /** To a board column; `before`/`after` a task of that column (manual order), else at its end. */
  | { type: 'setStatus'; ids: string[]; status: string; before?: string; after?: string }
  /** Replaces the columns; tasks of removed columns go to `reassign[removed id]` (default: the first column). */
  | { type: 'saveColumns'; columns: StatusColumn[]; reassign?: Record<string, string> }
  | { type: 'delete'; ids: string[] }
  | { type: 'move'; ids: string[]; listId: string; sectionId?: string | null; parentId?: string | null; index?: number | null }
  | { type: 'restore'; tasks: Task[] }
  | { type: 'addComment'; id: string; text: string }
  | { type: 'deleteComment'; id: string; commentId: string }
  | { type: 'removeAttachment'; id: string; attachmentId: string }
  | { type: 'saveList'; id?: string; name: string; color?: string | null }
  | { type: 'deleteList'; id: string }
  | { type: 'reorderLists'; ids: string[] }
  | { type: 'saveSection'; id?: string; listId: string; name: string; collapsed?: boolean }
  | { type: 'deleteSection'; id: string }
  | { type: 'reorderSections'; ids: string[] }
  | { type: 'saveLabel'; name: string; newName?: string; color?: string | null }
  /** Tasks with `name` get `into` instead; `name` is removed. */
  | { type: 'mergeLabel'; name: string; into: string }
  | { type: 'deleteLabel'; name: string }
  | { type: 'saveFilter'; id?: string; name: string; query: string; color?: string | null }
  | { type: 'deleteFilter'; id: string }
  | { type: 'reorderFilters'; ids: string[] }
  | { type: 'saveSettings'; settings: TodoSettings }
  | { type: 'clearCompleted'; listId?: string | null };

export interface Applied {
  created?: string | null;
  rescheduled: { id: string; date: string }[];
  count: number;
}

export interface TodoApplyResult {
  applied: Applied;
  snapshot: Snapshot;
}

export type TokenKind = 'date' | 'deadline' | 'list' | 'section' | 'label' | 'priority';

/** A recognised piece of quick-add text; offsets are UTF-16 (JS string indices). */
export interface QuickToken {
  kind: TokenKind;
  start: number;
  end: number;
  text: string;
  /** List/section id, label name, `p1`, `YYYY-MM-DD`. */
  value: string;
  new?: boolean;
}

export interface QuickAdd {
  content: string;
  listId?: string | null;
  sectionId?: string | null;
  labels: string[];
  newLabels: string[];
  priority?: Priority | null;
  due?: Due | null;
  noDate: boolean;
  deadline?: string;
  tokens: QuickToken[];
}

export interface CompletedEntry {
  task: TaskView;
  at: number;
  /** A completion of a recurring task (the task itself is still open). */
  recurring: boolean;
  occurrence?: string;
}

export interface TodoAttachResult {
  added: Attachment[];
  /** Files that couldn't be attached, with the reason. */
  errors: string[];
  snapshot: Snapshot;
}

/** Largest attachment (checked again in Rust). */
export const MAX_ATTACHMENT_BYTES = 50 * 1024 * 1024;

/** JSON safe for an HTTP header value (non-ASCII as `\u` escapes, which JSON parsers accept). */
const asciiJson = (v: unknown) => JSON.stringify(v).replace(/[\u007f-￿]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`);

/** `root`: the open project (its list is in scope), or null. `allDone`: snapshots with every completed task (not only the last week's). */
export const todosApi = {
  snapshot: (root: string | null, allDone = false) => invoke<Snapshot>('todos_snapshot', { root, allDone }),
  /** Tasks by id, however old (the snapshot leaves out tasks completed more than a week ago). */
  get: (ids: string[]) => invoke<TaskView[]>('todos_get', { ids }),
  apply: (root: string | null, op: TodoOp, allDone = false) => invoke<TodoApplyResult>('todos_apply', { root, op, allDone }),
  parse: (root: string | null, text: string, listId?: string | null, ignore?: string[]) => invoke<QuickAdd>('todos_parse', { root, text, listId, ignore }),
  parseDate: (text: string, dateOnly?: boolean) => invoke<Due | null>('todos_parse_date', { text, dateOnly }),
  filter: (query: string) => invoke<FilterGroup[]>('todos_filter', { query }),
  completed: (listId?: string | null, before?: number, limit?: number) => invoke<CompletedEntry[]>('todos_completed', { listId, before, limit }),
  notifyTest: () => invoke<void>('todos_notify_test'),
  /** Native file dialog (opened in Rust) → copies attached to the task; `null` when cancelled. */
  attachPick: (root: string | null, taskId: string, allDone = false) => invoke<TodoAttachResult | null>('todos_attachment_pick', { root, taskId, allDone }),
  /** Dropped or pasted bytes as an attachment. */
  attachBytes: (root: string | null, taskId: string, name: string, mime: string, bytes: Uint8Array, allDone = false) =>
    invoke<TodoAttachResult>('todos_attachment_upload', bytes, { headers: { 'x-meta': asciiJson({ root, taskId, name, mime: mime || undefined, allDone }) } }),
  /** The attachment's bytes (image previews). */
  attachmentBytes: (taskId: string, attachmentId: string) => invoke<ArrayBuffer>('todos_attachment_read', { taskId, attachmentId }),
  /** Opens with the default app; `false` = a program, revealed in the file manager instead. */
  openAttachment: (taskId: string, attachmentId: string) => invoke<boolean>('todos_attachment_open', { taskId, attachmentId }),
  revealAttachment: (taskId: string, attachmentId: string) => invoke<void>('todos_attachment_reveal', { taskId, attachmentId }),
};
