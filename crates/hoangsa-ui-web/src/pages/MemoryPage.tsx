import { useState } from "react";
import { Search, FileText, Archive, Wrench, Activity } from "lucide-react";
import { cn } from "@/lib/utils";
import { type MemoryHealthRes } from "@/api";
import { HealthTab } from "@/pages/memory/HealthTab";
import { RecallTab } from "@/pages/memory/RecallTab";
import { FilesTab } from "@/pages/memory/FilesTab";
import { ArchiveTab } from "@/pages/memory/ArchiveTab";
import { SkillsTab } from "@/pages/memory/SkillsTab";

type SubTab = "recall" | "files" | "archive" | "skills" | "health";

const SUB_TABS: Array<{
  id: SubTab;
  label: string;
  icon: React.ComponentType<{ className?: string }>;
  /** True if this sub-tab is meaningful only when the daemon is up. */
  needsDaemon: boolean;
}> = [
  { id: "recall", label: "Recall", icon: Search, needsDaemon: true },
  { id: "files", label: "Files", icon: FileText, needsDaemon: false },
  { id: "archive", label: "Archive", icon: Archive, needsDaemon: true },
  { id: "skills", label: "Skills", icon: Wrench, needsDaemon: true },
  { id: "health", label: "Health", icon: Activity, needsDaemon: false },
];

export function MemoryPage({
  status,
  onStatusChange,
}: {
  status: MemoryHealthRes | null;
  onStatusChange: (s: MemoryHealthRes) => void;
}) {
  // Default to Files: it's the only sub-tab that's still useful when the
  // daemon is down, so picking it as the entry point avoids landing on a
  // disabled tab during a cold session.
  const [active, setActive] = useState<SubTab>("files");

  if (!status) return <div className="bg-muted h-32 animate-pulse rounded" />;

  return (
    <div className="space-y-4">
      <nav className="flex flex-wrap gap-1 border-b">
        {SUB_TABS.map((t) => {
          const disabled = t.needsDaemon && !status.ok;
          const isActive = active === t.id;
          return (
            <button
              key={t.id}
              type="button"
              onClick={() => !disabled && setActive(t.id)}
              disabled={disabled}
              className={cn(
                "flex items-center gap-1.5 border-b-2 px-3 py-2 text-sm font-medium transition-colors",
                "-mb-px",
                isActive
                  ? "border-foreground text-foreground"
                  : "border-transparent text-muted-foreground hover:text-foreground",
                disabled && "cursor-not-allowed opacity-40 hover:text-muted-foreground",
              )}
              title={disabled ? "Cần daemon up" : undefined}
            >
              <t.icon className="size-3.5" />
              {t.label}
            </button>
          );
        })}
      </nav>

      <div>
        {active === "recall" && <RecallTab />}
        {active === "files" && <FilesTab status={status} />}
        {active === "archive" && <ArchiveTab />}
        {active === "skills" && <SkillsTab />}
        {active === "health" && (
          <HealthTab status={status} onStatusChange={onStatusChange} />
        )}
      </div>
    </div>
  );
}
