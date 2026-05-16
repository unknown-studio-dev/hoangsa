// Token is read from `?t=...` on first load and shared across tabs through
// a sessionStorage cache so duplicating the tab still works (locked Q5: same
// token shared, expires when CLI process exits).
const TOKEN_KEY = "hoangsa-ui:token";

export function readToken(): string {
  const params = new URLSearchParams(window.location.search);
  const fromUrl = params.get("t");
  if (fromUrl) {
    sessionStorage.setItem(TOKEN_KEY, fromUrl);
    return fromUrl;
  }
  return sessionStorage.getItem(TOKEN_KEY) ?? "";
}

const TOKEN = readToken();

function withToken(path: string): string {
  const sep = path.includes("?") ? "&" : "?";
  return `${path}${sep}t=${TOKEN}`;
}

async function request<T>(path: string, init?: RequestInit): Promise<T> {
  const res = await fetch(withToken(path), {
    ...init,
    headers: {
      ...(init?.body ? { "Content-Type": "application/json" } : {}),
      ...(init?.headers ?? {}),
    },
  });
  if (!res.ok) {
    let body: unknown = null;
    try {
      body = await res.json();
    } catch {
      // ignore — fall through to status-only error
    }
    const msg =
      (body as { error?: string } | null)?.error ?? `HTTP ${res.status}`;
    const err = new Error(msg) as Error & { status: number };
    err.status = res.status;
    throw err;
  }
  if (res.status === 204) return undefined as T;
  return res.json();
}

export const api = {
  health: () => request<HealthRes>("/api/health"),
  configEffective: () => request<ConfigEffectiveRes>("/api/config/effective"),
  configDiff: (body: PatchBody) =>
    request<DiffRes>("/api/config/diff", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  configApply: (body: PatchBody) =>
    request<ApplyRes>("/api/config/apply", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  rulesList: () => request<RulesListRes>("/api/rules"),
  rulesAdd: (body: { rule: Rule; expected_mtime_ms?: number }) =>
    request<RulesMutRes>("/api/rules", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  rulesToggle: (id: string, enabled: boolean, mtime?: number) =>
    request<RulesMutRes>(`/api/rules/${encodeURIComponent(id)}/toggle`, {
      method: "POST",
      body: JSON.stringify({ enabled, expected_mtime_ms: mtime }),
    }),
  rulesRemove: (id: string, mtime?: number) =>
    request<RulesMutRes>(`/api/rules/${encodeURIComponent(id)}`, {
      method: "DELETE",
      body: JSON.stringify({ expected_mtime_ms: mtime }),
    }),
  rulesSyncDefaults: (mtime?: number) =>
    request<SyncDefaultsRes>("/api/rules/sync-defaults", {
      method: "POST",
      body: JSON.stringify({ expected_mtime_ms: mtime }),
    }),
  addonsList: () => request<AddonsListRes>("/api/addons"),
  memoryHealth: () => request<MemoryHealthRes>("/api/memory/health"),
  memoryRestart: () =>
    request<MemoryRestartRes>("/api/memory/restart", { method: "POST" }),
  projectsList: () => request<ProjectsListRes>("/api/projects"),
  projectsCurrent: () => request<ProjectSummary>("/api/projects/current"),
  projectsRegister: (path: string, name?: string) =>
    request<{ project: ProjectEntry | null }>("/api/projects", {
      method: "POST",
      body: JSON.stringify({ path, name }),
    }),
  projectsSwitch: (body: { slug?: string; path?: string }) =>
    request<ProjectSwitchRes>("/api/projects/switch", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  projectsRemove: (slug: string) =>
    request<{ slug: string; removed: boolean }>(
      `/api/projects/${encodeURIComponent(slug)}`,
      { method: "DELETE" }
    ),

  // Memory daemon proxy. All POSTs except memoryFiles, which is the
  // FS-direct degraded read and stays useful when the daemon is down.
  memoryFiles: () => request<MemoryFilesRes>("/api/memory/files"),
  memoryShow: () =>
    request<ToolOutput<MemoryShowData>>("/api/memory/show", {
      method: "POST",
      body: "{}",
    }),
  memoryRecall: (body: MemoryRecallReq) =>
    request<ToolOutput<MemoryRecallData>>("/api/memory/recall", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  memoryRememberFact: (body: RememberFactReq) =>
    request<ToolOutput<MemoryWriteData>>("/api/memory/fact", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  memoryRememberLesson: (body: RememberLessonReq) =>
    request<ToolOutput<MemoryWriteData>>("/api/memory/lesson", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  memoryRememberPreference: (body: RememberPreferenceReq) =>
    request<ToolOutput<MemoryWriteData>>("/api/memory/preference", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  memoryRemove: (body: {
    kind: "fact" | "lesson" | "preference";
    query: string;
  }) =>
    request<ToolOutput<unknown>>("/api/memory/remove", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  memoryArchiveSearch: (body: ArchiveSearchReq) =>
    request<ToolOutput<ArchiveHit[]>>("/api/memory/archive/search", {
      method: "POST",
      body: JSON.stringify(body),
    }),
  memorySkills: () => request<ToolOutput<SkillEntry[]>>("/api/memory/skills"),
};

// ── Types ──────────────────────────────────────────────────────────────

export type HealthRes = {
  ok: boolean;
  project_dir: string;
  project_slug: string;
  project_name: string;
  global_dir: string;
};

export type ConfigEffectiveRes = {
  global: unknown | null;
  project: unknown | null;
  effective: unknown;
  sources: Record<string, "global" | "project">;
  global_path: string;
  project_path: string;
};

type PatchBody = {
  layer: "global" | "project";
  patch: unknown;
  expected_mtime_ms?: number;
};

type DiffRes = {
  before: unknown;
  after: unknown;
  mtime_ms: number | null;
  path: string;
};

type ApplyRes = {
  after: unknown;
  mtime_ms: number | null;
  path: string;
};

export type Rule = {
  id: string;
  name: string;
  enabled: boolean;
  enforcement: "hook" | "preflight" | "prompt";
  matcher: string;
  conditions: Condition[];
  action: "block" | "warn";
  message: string;
  stateful?: string | null;
};

export type Condition = {
  field: string;
  op: "glob" | "regex" | "contains" | "not_contains" | "starts_with";
  value: string;
};

export type RulesListRes = {
  rules: Rule[];
  count: number;
  enabled: number;
  disabled: number;
  version?: string;
  initialized?: false;
  mtime_ms: number | null;
};

export type RulesMutRes = {
  rules: Rule[];
  mtime_ms: number | null;
};

export type SyncDefaultsRes = {
  added: string[];
  replaced: string[];
  user_kept: string[];
  rules: Rule[];
  mtime_ms: number | null;
};

export type AddonsListRes = {
  available: AddonInfo[];
  active: string[];
  hoangsa_root: string;
};

export type AddonInfo = {
  name: string;
  description?: string;
  frameworks?: string[];
  test_frameworks?: string[];
  priority?: number;
};

export type MemoryHealthRes = {
  ok: boolean;
  socket_exists: boolean;
  socket_path: string;
  project_slug: string;
};

export type MemoryRestartRes = {
  killed: boolean;
  message: string;
};

export type ProjectEntry = {
  slug: string;
  path: string;
  name: string;
  registered_at: number;
  last_used_at: number;
  exists: boolean;
};

export type ProjectSummary = {
  slug: string;
  path: string;
  name: string;
};

export type ProjectsListRes = {
  projects: ProjectEntry[];
  orphan_slugs: string[];
  current: ProjectSummary;
};

export type ProjectSwitchRes = {
  previous: ProjectSummary;
  current: ProjectSummary;
};

// ── Memory tool envelope ──────────────────────────────────────────────
// Mirrors `hoangsa_memory_mcp::proto::ToolOutput`. `data` is the
// machine-readable result (shape varies by tool); `text` is the daemon's
// human-rendered form, useful as a fallback when the UI doesn't have a
// custom view yet.

export type ToolOutput<D> = {
  data: D;
  text: string;
  isError: boolean;
};

export type MemoryFileSnapshot = {
  path: string;
  body: string | null;
  bytes: number | null;
};

export type MemoryFilesRes = {
  user: MemoryFileSnapshot;
  memory: MemoryFileSnapshot;
  lessons: MemoryFileSnapshot;
};

export type MemoryShowData = {
  memory_md: string | null;
  lessons_md: string | null;
  user_md: string | null;
};

export type MemoryRecallReq = {
  query: string;
  top_k?: number;
  scope?: "curated" | "archive" | "all";
  tags?: string[];
  detail?: boolean;
};

export type RecallChunk = {
  id: string;
  path: string;
  line: number;
  span: [number, number];
  symbol: string | null;
  preview: string;
  body: string;
  source: string;
  score: number;
};

export type MemoryRecallData = {
  chunks: RecallChunk[];
  synthesized: string | null;
  correlation_id: string;
};

export type RememberFactReq = {
  text: string;
  tags?: string[];
  scope?: "always" | "on-demand";
};

export type RememberLessonReq = {
  trigger: string;
  advice: string;
};

export type RememberPreferenceReq = {
  text: string;
  tags?: string[];
};

export type MemoryWriteData = {
  text?: string;
  tags?: string[];
  trigger?: string;
  advice?: string;
  path: string;
  staged?: boolean;
};

export type ArchiveSearchReq = {
  query: string;
  top_k?: number;
  project?: string;
  topic?: string;
};

export type ArchiveHit = {
  id: string;
  distance: number;
  text: string | null;
  metadata: Record<string, unknown> | null;
};

export type SkillEntry = {
  slug: string;
  description: string;
  // The daemon may include more fields; we only render slug + description.
  [k: string]: unknown;
};
