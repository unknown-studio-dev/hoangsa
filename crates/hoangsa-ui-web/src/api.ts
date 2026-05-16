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
