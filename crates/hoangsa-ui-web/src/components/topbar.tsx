import { Activity } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { StatusDot } from "@/components/status-dot";
import type { MemoryHealthRes } from "@/api";

const TITLES: Record<string, string> = {
  dashboard: "Overview",
  config: "Configuration",
  rules: "Protection Rules",
  addons: "Addons",
  memory: "Memory daemon",
};

export function Topbar({
  tab,
  memory,
  connected,
}: {
  tab: string;
  memory: MemoryHealthRes | null;
  connected: boolean;
}) {
  const tone = memory?.ok ? "ok" : memory ? "warn" : "idle";
  const memoryLabel = memory ? (memory.ok ? "Memory live" : "Memory down") : "Memory …";

  return (
    <header className="bg-background/60 supports-[backdrop-filter]:bg-background/40 border-b backdrop-blur">
      <div className="flex h-14 items-center justify-between gap-4 px-6">
        <div className="flex items-center gap-3">
          <h1 className="text-base font-semibold tracking-tight">
            {TITLES[tab] ?? "hoangsa"}
          </h1>
        </div>
        <div className="flex items-center gap-3">
          <div className="bg-card flex items-center gap-2 rounded-md border px-3 py-1.5 text-xs">
            <StatusDot tone={tone} />
            <span className="font-medium">{memoryLabel}</span>
          </div>
          <Badge
            variant={connected ? "secondary" : "destructive"}
            className="gap-1.5"
          >
            <Activity className="size-3" />
            {connected ? "connected" : "offline"}
          </Badge>
        </div>
      </div>
    </header>
  );
}
