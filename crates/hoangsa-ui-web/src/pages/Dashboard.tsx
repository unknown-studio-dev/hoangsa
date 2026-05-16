import { useEffect, useState } from "react";
import {
  Shield,
  Puzzle,
  Database,
  Layers,
  Folder,
  Clock,
  ShieldCheck,
  CircleDot,
  ArrowRight,
} from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { StatsGrid, type StatItem } from "@/components/stats-grid";
import { StatusDot } from "@/components/status-dot";
import { api, type HealthRes, type MemoryHealthRes } from "@/api";

type Counts = {
  rules: { total: number; enabled: number };
  addons: { total: number; active: number };
};

export function Dashboard({
  health,
  memory,
  onNavigate,
}: {
  health: HealthRes | null;
  memory: MemoryHealthRes | null;
  onNavigate: (tab: string) => void;
}) {
  const [counts, setCounts] = useState<Counts>({
    rules: { total: 0, enabled: 0 },
    addons: { total: 0, active: 0 },
  });

  useEffect(() => {
    api.rulesList().then((r) =>
      setCounts((p) => ({
        ...p,
        rules: { total: r.count, enabled: r.enabled },
      })),
    );
    api.addonsList().then((a) =>
      setCounts((p) => ({
        ...p,
        addons: { total: a.available.length, active: a.active.length },
      })),
    );
  }, []);

  const items: StatItem[] = [
    {
      icon: Shield,
      label: "Rules enabled",
      value: `${counts.rules.enabled}/${counts.rules.total || "—"}`,
      delta: "đang bảo vệ",
      tone: "emerald",
      onClick: () => onNavigate("rules"),
    },
    {
      icon: Puzzle,
      label: "Addons active",
      value: `${counts.addons.active}/${counts.addons.total || "—"}`,
      delta: "worker rules",
      tone: "violet",
      onClick: () => onNavigate("addons"),
    },
    {
      icon: Database,
      label: "Memory daemon",
      value: memory ? (memory.ok ? "Live" : "Down") : "…",
      delta: memory?.project_slug ?? "",
      tone: memory?.ok ? "sky" : "amber",
      onClick: () => onNavigate("memory"),
    },
    {
      icon: Layers,
      label: "Layered config",
      value: "merged",
      delta: "global → project",
      tone: "amber",
      onClick: () => onNavigate("config"),
    },
  ];

  return (
    <div className="space-y-4">
      <StatsGrid items={items} />

      <div className="grid gap-4 lg:grid-cols-2">
        <ProjectCard health={health} />
        <DaemonHealthCard memory={memory} />
      </div>

      <Card>
        <CardHeader>
          <CardTitle className="flex items-center gap-2 text-sm">
            <ArrowRight className="size-4 text-violet-500" />
            Quick actions
          </CardTitle>
        </CardHeader>
        <CardContent className="space-y-1">
          <QuickRow
            icon={Shield}
            tone="emerald"
            title="Quản lý luật bảo vệ"
            desc="Bật/tắt rule, sync default mới khi nâng cấp binary"
            onClick={() => onNavigate("rules")}
          />
          <QuickRow
            icon={Puzzle}
            tone="violet"
            title="Bật tiện ích theo framework"
            desc="Worker rules cho stack đang dùng"
            onClick={() => onNavigate("addons")}
          />
          <QuickRow
            icon={Database}
            tone="sky"
            title="Kiểm tra memory daemon"
            desc="Xem trạng thái + restart nếu cần"
            onClick={() => onNavigate("memory")}
          />
        </CardContent>
      </Card>
    </div>
  );
}

function ProjectCard({ health }: { health: HealthRes | null }) {
  return (
    <Card className="overflow-hidden">
      <CardHeader className="border-b">
        <CardTitle className="flex items-center gap-2 text-sm">
          <Folder className="size-4 text-amber-500" />
          Workspace
        </CardTitle>
      </CardHeader>
      <CardContent className="grid grid-cols-1 gap-x-4 gap-y-5 pt-6 sm:grid-cols-2">
        <Field
          icon={<Folder className="text-muted-foreground size-3.5" />}
          label="Project dir"
        >
          <span className="font-mono text-xs break-all">
            {health?.project_dir.replace(/^\/Users\/[^/]+/, "~") ?? "…"}
          </span>
        </Field>
        <Field
          icon={<Folder className="text-muted-foreground size-3.5" />}
          label="Global dir"
        >
          <span className="font-mono text-xs break-all">
            {health?.global_dir.replace(/^\/Users\/[^/]+/, "~") ?? "…"}
          </span>
        </Field>
      </CardContent>
    </Card>
  );
}

function DaemonHealthCard({ memory }: { memory: MemoryHealthRes | null }) {
  const tone = memory?.ok ? "ok" : memory ? "warn" : "idle";
  return (
    <Card className="overflow-hidden">
      <CardHeader className="border-b">
        <CardTitle className="flex items-center gap-2 text-sm">
          <ShieldCheck className="size-4 text-emerald-500" />
          Service health
        </CardTitle>
      </CardHeader>
      <CardContent className="grid grid-cols-2 gap-x-4 gap-y-5 pt-6 sm:grid-cols-3">
        <Field
          icon={<StatusDot tone={tone} className="-ml-0.5" />}
          label="Memory"
        >
          <span
            className={
              memory?.ok ? "text-foreground" : "text-amber-600"
            }
          >
            {memory ? (memory.ok ? "Connectable" : "Unreachable") : "…"}
          </span>
        </Field>
        <Field
          icon={<CircleDot className="text-muted-foreground size-3.5" />}
          label="Socket"
        >
          {memory?.socket_exists ? "exists" : memory ? "absent" : "…"}
        </Field>
        <Field
          icon={<Clock className="text-muted-foreground size-3.5" />}
          label="Slug"
        >
          <span className="font-mono text-xs">
            {memory?.project_slug ?? "…"}
          </span>
        </Field>
      </CardContent>
    </Card>
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

function QuickRow({
  icon: Icon,
  tone,
  title,
  desc,
  onClick,
}: {
  icon: typeof Shield;
  tone: "emerald" | "violet" | "sky";
  title: string;
  desc: string;
  onClick: () => void;
}) {
  const TONE: Record<string, string> = {
    emerald: "bg-emerald-500/10 text-emerald-500 ring-emerald-500/20",
    violet: "bg-violet-500/10 text-violet-500 ring-violet-500/20",
    sky: "bg-sky-500/10 text-sky-500 ring-sky-500/20",
  };
  return (
    <button
      onClick={onClick}
      className="hover:bg-muted/50 group flex w-full items-center gap-3 rounded-md p-3 text-left transition-colors"
    >
      <div
        className={
          "grid size-9 shrink-0 place-items-center rounded-md ring-1 " + TONE[tone]
        }
      >
        <Icon className="size-4" />
      </div>
      <div className="min-w-0 flex-1">
        <div className="text-sm font-medium">{title}</div>
        <div className="text-muted-foreground text-xs">{desc}</div>
      </div>
      <ArrowRight className="text-muted-foreground size-4 transition-transform group-hover:translate-x-0.5" />
    </button>
  );
}
