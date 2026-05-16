import { useEffect, useState } from "react";
import { toast } from "sonner";
import {
  RefreshCw,
  Settings2,
  Code2,
  Cpu,
  ListTodo,
  Layers,
  ChevronDown,
  ChevronRight,
  FileText,
} from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { api, type ConfigEffectiveRes } from "@/api";

const FRIENDLY: Record<string, string> = {
  "preferences.lang": "Ngôn ngữ giao tiếp",
  "preferences.spec_lang": "Ngôn ngữ viết spec",
  "preferences.tech_stack": "Stack công nghệ",
  "preferences.interaction_level": "Mức tương tác",
  "preferences.review_style": "Cách review",
  "preferences.research_mode": "Chế độ research",
  "preferences.research_scope": "Phạm vi research",
  "preferences.auto_taste": "Tự chạy /taste",
  "preferences.auto_plate": "Tự chạy /plate",
  "preferences.auto_serve": "Tự chạy /serve",
  "preferences.auto_compact": "Tự compact context",
  "preferences.auto_compact_interval": "Compact interval",
  "preferences.auto_compact_cooldown_secs": "Compact cooldown (s)",
  "preferences.context_mode": "Chế độ context",
  "preferences.memory_strict": "Memory strict mode",
  "preferences.simplify_pass": "Pass simplify",
  "preferences.quality_gate": "Quality gate",
  "preferences.test_runs": "Số lần chạy test",
  "codebase.frameworks": "Frameworks",
  "codebase.databases": "Databases",
  "codebase.linters": "Linters",
  "codebase.formatters": "Formatters",
  "codebase.testing": "Cấu hình testing",
  "codebase.active_addons": "Tiện ích đang dùng",
  "codebase.package_manager": "Package manager",
  "codebase.entry_points": "Entry points",
  "codebase.scripts": "Scripts",
  "codebase.api_style": "API style",
  "codebase.styling": "Styling",
  "codebase.bundler": "Bundler",
  "codebase.state_management": "State management",
  "codebase.auth": "Auth",
  "codebase.cloud": "Cloud",
  "codebase.ci": "CI",
  "codebase.git_convention": "Git convention",
  "codebase.git_hooks": "Git hooks",
  "codebase.infra": "Infra",
  "codebase.runtime_versions": "Runtime versions",
  "codebase.orm": "ORM",
  "codebase.documentation": "Docs",
  "codebase.message_queue": "Message queue",
  "codebase.monitoring": "Monitoring",
  "codebase.monorepo": "Monorepo?",
  "codebase.packages": "Packages",
};

const SECTIONS: Array<{
  key: string;
  label: string;
  desc: string;
  icon: typeof Settings2;
  tone: string;
}> = [
  {
    key: "preferences",
    label: "Preferences",
    desc: "Ngôn ngữ, mức tương tác, hành vi auto",
    icon: Settings2,
    tone: "bg-emerald-500/10 text-emerald-500 ring-emerald-500/20",
  },
  {
    key: "codebase",
    label: "Codebase",
    desc: "Stack, framework, công cụ build/test",
    icon: Code2,
    tone: "bg-sky-500/10 text-sky-500 ring-sky-500/20",
  },
  {
    key: "model_overrides",
    label: "Model overrides",
    desc: "Pin model cho từng worker",
    icon: Cpu,
    tone: "bg-violet-500/10 text-violet-500 ring-violet-500/20",
  },
  {
    key: "task_manager",
    label: "Task manager",
    desc: "Tích hợp Linear / Jira / ClickUp",
    icon: ListTodo,
    tone: "bg-amber-500/10 text-amber-500 ring-amber-500/20",
  },
];

export function ConfigPage() {
  const [data, setData] = useState<ConfigEffectiveRes | null>(null);

  const refresh = async () => {
    try {
      setData(await api.configEffective());
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  if (!data) return <div className="bg-muted h-32 animate-pulse rounded" />;

  const eff = data.effective as Record<string, unknown> | null;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-muted-foreground text-sm">
          Giá trị effective sau khi merge global → project. Badge bên cạnh giá
          trị cho biết nó đến từ đâu.
        </p>
        <Button variant="outline" size="sm" onClick={refresh}>
          <RefreshCw className="size-3.5" />
          Reload
        </Button>
      </div>

      <Card className="overflow-hidden">
        <CardHeader className="border-b">
          <CardTitle className="flex items-center gap-2 text-sm">
            <FileText className="size-4 text-amber-500" />
            Source files
          </CardTitle>
        </CardHeader>
        <CardContent className="grid grid-cols-1 gap-x-4 gap-y-5 pt-6 sm:grid-cols-2">
          <Field
            icon={<SourceBadge source="project" />}
            label="Project layer"
          >
            <span
              className={
                "font-mono text-xs break-all " +
                (data.project ? "" : "text-muted-foreground")
              }
            >
              {data.project_path}
              {!data.project && " (chưa có)"}
            </span>
          </Field>
          <Field icon={<SourceBadge source="global" />} label="Global layer">
            <span
              className={
                "font-mono text-xs break-all " +
                (data.global ? "" : "text-muted-foreground")
              }
            >
              {data.global_path}
              {!data.global && " (chưa có)"}
            </span>
          </Field>
        </CardContent>
      </Card>

      {SECTIONS.map((s) => {
        const value = eff ? (eff[s.key] as unknown) : undefined;
        if (value === undefined) return null;
        return (
          <Section
            key={s.key}
            sectionKey={s.key}
            label={s.label}
            desc={s.desc}
            Icon={s.icon}
            tone={s.tone}
            value={value}
            sources={data.sources}
          />
        );
      })}

      {eff && (
        <OtherKeys
          eff={eff}
          known={SECTIONS.map((s) => s.key)}
          sources={data.sources}
        />
      )}
    </div>
  );
}

function Section({
  sectionKey,
  label,
  desc,
  Icon,
  tone,
  value,
  sources,
}: {
  sectionKey: string;
  label: string;
  desc: string;
  Icon: typeof Settings2;
  tone: string;
  value: unknown;
  sources: Record<string, "global" | "project">;
}) {
  return (
    <Card className="overflow-hidden">
      <CardHeader className="border-b">
        <div className="flex items-start gap-3">
          <div
            className={
              "grid size-9 shrink-0 place-items-center rounded-md ring-1 " + tone
            }
          >
            <Icon className="size-4" />
          </div>
          <div className="min-w-0">
            <CardTitle className="text-sm">{label}</CardTitle>
            <p className="text-muted-foreground mt-0.5 text-xs">{desc}</p>
          </div>
        </div>
      </CardHeader>
      <CardContent className="pt-4">
        <KVList value={value} parentPath={sectionKey} sources={sources} />
      </CardContent>
    </Card>
  );
}

function OtherKeys({
  eff,
  known,
  sources,
}: {
  eff: Record<string, unknown>;
  known: string[];
  sources: Record<string, "global" | "project">;
}) {
  const others = Object.entries(eff).filter(([k]) => !known.includes(k));
  if (others.length === 0) return null;
  return (
    <Card className="overflow-hidden">
      <CardHeader className="border-b">
        <div className="flex items-start gap-3">
          <div className="bg-muted text-muted-foreground ring-border grid size-9 shrink-0 place-items-center rounded-md ring-1">
            <Layers className="size-4" />
          </div>
          <div>
            <CardTitle className="text-sm">Other</CardTitle>
            <p className="text-muted-foreground mt-0.5 text-xs">
              Các trường top-level chưa được nhóm
            </p>
          </div>
        </div>
      </CardHeader>
      <CardContent className="space-y-1 pt-4">
        {others.map(([k, v]) => (
          <Row key={k} keyName={k} value={v} path={k} sources={sources} />
        ))}
      </CardContent>
    </Card>
  );
}

function KVList({
  value,
  parentPath,
  sources,
}: {
  value: unknown;
  parentPath: string;
  sources: Record<string, "global" | "project">;
}) {
  if (value === null || value === undefined) {
    return (
      <div className="text-muted-foreground text-sm">không có giá trị</div>
    );
  }
  if (typeof value !== "object" || Array.isArray(value)) {
    return (
      <div className="flex items-baseline gap-2">
        <ValueDisplay value={value} />
        <SourceBadge source={sources[parentPath]} />
      </div>
    );
  }
  const entries = Object.entries(value as Record<string, unknown>);
  if (entries.length === 0) {
    return (
      <div className="text-muted-foreground text-sm italic">trống</div>
    );
  }
  return (
    <div className="space-y-0.5">
      {entries.map(([k, v]) => (
        <Row
          key={k}
          keyName={k}
          value={v}
          path={`${parentPath}.${k}`}
          sources={sources}
        />
      ))}
    </div>
  );
}

function Row({
  keyName,
  value,
  path,
  sources,
}: {
  keyName: string;
  value: unknown;
  path: string;
  sources: Record<string, "global" | "project">;
}) {
  const isObject =
    value !== null && typeof value === "object" && !Array.isArray(value);
  const [open, setOpen] = useState(false);
  const label = FRIENDLY[path] ?? humanize(keyName);

  if (isObject) {
    const childCount = Object.keys(value as object).length;
    return (
      <div className="border-border/40 rounded-md border">
        <button
          onClick={() => setOpen((v) => !v)}
          className="hover:bg-muted/40 flex w-full items-center gap-2 px-3 py-2 text-left text-sm"
        >
          {open ? (
            <ChevronDown className="text-muted-foreground size-3.5" />
          ) : (
            <ChevronRight className="text-muted-foreground size-3.5" />
          )}
          <span className="font-medium">{label}</span>
          <Badge variant="outline" className="text-[10px] tabular-nums">
            {childCount}
          </Badge>
          <span className="text-muted-foreground ml-auto font-mono text-[11px]">
            {keyName}
          </span>
        </button>
        {open && (
          <div className="border-border/40 border-t px-3 pb-3 pt-1">
            <KVList value={value} parentPath={path} sources={sources} />
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="flex items-baseline justify-between gap-3 rounded-md px-2 py-1.5 hover:bg-muted/30">
      <div className="min-w-0">
        <div className="text-sm font-medium">{label}</div>
        <div className="text-muted-foreground font-mono text-[11px]">
          {keyName}
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-2">
        <ValueDisplay value={value} />
        <SourceBadge source={sources[path]} />
      </div>
    </div>
  );
}

function ValueDisplay({ value }: { value: unknown }) {
  if (value === null) {
    return (
      <span className="text-muted-foreground text-sm italic">null</span>
    );
  }
  if (typeof value === "boolean") {
    return (
      <Badge
        className={
          value
            ? "border-emerald-500/20 bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/10 text-[10px] uppercase tracking-wider"
            : "border-border bg-muted text-muted-foreground hover:bg-muted text-[10px] uppercase tracking-wider"
        }
      >
        {value ? "true" : "false"}
      </Badge>
    );
  }
  if (Array.isArray(value)) {
    if (value.length === 0) {
      return (
        <span className="text-muted-foreground text-sm italic">trống</span>
      );
    }
    return (
      <div className="flex max-w-xs flex-wrap justify-end gap-1">
        {value.map((v, i) => (
          <Badge key={i} variant="secondary" className="font-mono text-[11px]">
            {String(v)}
          </Badge>
        ))}
      </div>
    );
  }
  if (typeof value === "string") {
    return (
      <code className="bg-muted rounded px-1.5 py-0.5 text-sm">{value}</code>
    );
  }
  return <code className="text-sm tabular-nums">{String(value)}</code>;
}

function SourceBadge({ source }: { source?: "global" | "project" }) {
  if (!source) return null;
  if (source === "project") {
    return (
      <Badge className="border-emerald-500/20 bg-emerald-500/10 text-emerald-600 hover:bg-emerald-500/10 text-[10px] px-1.5 uppercase tracking-wider">
        project
      </Badge>
    );
  }
  return (
    <Badge className="border-sky-500/20 bg-sky-500/10 text-sky-600 hover:bg-sky-500/10 text-[10px] px-1.5 uppercase tracking-wider">
      global
    </Badge>
  );
}

function Field({
  icon,
  label,
  children,
}: {
  icon: React.ReactNode;
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-1">
      <div className="text-muted-foreground flex items-center gap-1.5 text-[10px] uppercase tracking-wider">
        {icon}
        {label}
      </div>
      <div className="text-sm font-medium">{children}</div>
    </div>
  );
}

function humanize(key: string): string {
  return key
    .split(/[_\s-]/)
    .filter(Boolean)
    .map((w) => w.charAt(0).toUpperCase() + w.slice(1))
    .join(" ");
}
