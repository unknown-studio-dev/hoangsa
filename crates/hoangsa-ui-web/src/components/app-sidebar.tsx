import {
  LayoutDashboard,
  Settings2,
  Shield,
  Puzzle,
  Database,
  type LucideIcon,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { ProjectSwitcher } from "@/components/project-switcher";
import type { ProjectSummary } from "@/api";

export type Tab = "dashboard" | "config" | "rules" | "addons" | "memory";

const ITEMS: { id: Tab; label: string; icon: LucideIcon }[] = [
  { id: "dashboard", label: "Dashboard", icon: LayoutDashboard },
  { id: "config", label: "Config", icon: Settings2 },
  { id: "rules", label: "Rules", icon: Shield },
  { id: "addons", label: "Addons", icon: Puzzle },
  { id: "memory", label: "Memory", icon: Database },
];

export function AppSidebar({
  active,
  onNavigate,
  currentProject,
  onSwitched,
}: {
  active: Tab;
  onNavigate: (tab: Tab) => void;
  currentProject: ProjectSummary | null;
  onSwitched: (next: ProjectSummary) => void;
}) {
  return (
    <aside className="bg-sidebar text-sidebar-foreground border-sidebar-border hidden h-full w-60 shrink-0 flex-col border-r md:flex">
      <div className="border-sidebar-border flex h-14 items-center gap-2.5 border-b px-5">
        <div className="grid size-8 place-items-center rounded-md bg-violet-500/15 text-violet-500 ring-1 ring-violet-500/30">
          <Shield className="size-4" />
        </div>
        <div className="flex flex-col leading-tight">
          <span className="text-sm font-semibold tracking-tight">hoangsa</span>
          <span className="text-muted-foreground text-[10px] uppercase tracking-wider">
            config console
          </span>
        </div>
      </div>
      <nav className="flex-1 space-y-0.5 px-2 py-3">
        {ITEMS.map(({ id, label, icon: Icon }) => {
          const isActive = active === id;
          return (
            <button
              key={id}
              onClick={() => onNavigate(id)}
              className={cn(
                "group flex w-full items-center gap-2.5 rounded-md px-3 py-2 text-sm font-medium transition-colors",
                isActive
                  ? "bg-sidebar-accent text-sidebar-accent-foreground"
                  : "text-muted-foreground hover:bg-sidebar-accent/60 hover:text-sidebar-accent-foreground",
              )}
            >
              <Icon
                className={cn(
                  "size-4 transition-colors",
                  isActive && "text-violet-500",
                )}
              />
              <span>{label}</span>
            </button>
          );
        })}
      </nav>
      <div className="border-sidebar-border border-t p-2">
        <ProjectSwitcher current={currentProject} onSwitched={onSwitched} />
      </div>
    </aside>
  );
}
