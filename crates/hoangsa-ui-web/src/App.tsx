import { useEffect, useState } from "react";
import { Toaster } from "@/components/ui/sonner";
import { AppSidebar, type Tab } from "@/components/app-sidebar";
import { Topbar } from "@/components/topbar";
import {
  api,
  type HealthRes,
  type MemoryHealthRes,
  type ProjectSummary,
} from "@/api";
import { Dashboard } from "@/pages/Dashboard";
import { ConfigPage } from "@/pages/ConfigPage";
import { RulesPage } from "@/pages/RulesPage";
import { AddonsPage } from "@/pages/AddonsPage";
import { MemoryPage } from "@/pages/MemoryPage";

export function App() {
  const [tab, setTab] = useState<Tab>("dashboard");
  const [health, setHealth] = useState<HealthRes | null>(null);
  const [memory, setMemory] = useState<MemoryHealthRes | null>(null);
  const [currentProject, setCurrentProject] = useState<ProjectSummary | null>(
    null,
  );
  const [error, setError] = useState<string | null>(null);

  // Initial boot: fetch health (which now includes project_slug + project_name)
  // and memory daemon status. Health derives the initial currentProject so the
  // sidebar shows the active label immediately, before the user opens the
  // switcher dialog.
  useEffect(() => {
    api
      .health()
      .then((h) => {
        setHealth(h);
        setCurrentProject({
          slug: h.project_slug,
          path: h.project_dir,
          name: h.project_name,
        });
      })
      .catch((e: Error) => setError(e.message));
    api.memoryHealth().then(setMemory).catch(() => {});
  }, []);

  // After a project switch, re-fetch memory daemon status (different socket
  // path) and force-remount all child pages so they refetch their data.
  const onSwitched = (next: ProjectSummary) => {
    setCurrentProject(next);
    api.memoryHealth().then(setMemory).catch(() => setMemory(null));
  };

  if (error) {
    return (
      <div className="bg-muted/10 grid h-screen place-items-center p-6">
        <div className="bg-card max-w-md rounded-xl border p-6 shadow-sm">
          <h1 className="mb-2 text-lg font-semibold">Không kết nối được server</h1>
          <p className="text-destructive mb-3 text-sm">{error}</p>
          <p className="text-muted-foreground text-sm">
            Token URL có thể đã hết hạn. Tắt và khởi động lại{" "}
            <code className="bg-muted rounded px-1">hoangsa-cli ui</code> để
            lấy URL mới.
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="bg-muted/10 flex h-screen overflow-hidden">
      <Toaster position="top-right" />
      <AppSidebar
        active={tab}
        onNavigate={setTab}
        currentProject={currentProject}
        onSwitched={onSwitched}
      />
      <div className="flex min-w-0 flex-1 flex-col">
        <Topbar tab={tab} memory={memory} connected={health !== null} />
        <main
          key={currentProject?.slug ?? "boot"}
          className="scrollbar-thin flex-1 overflow-y-auto p-4 sm:p-6"
        >
          {tab === "dashboard" && (
            <Dashboard
              health={
                currentProject && health
                  ? {
                      ...health,
                      project_dir: currentProject.path,
                    }
                  : health
              }
              memory={memory}
              onNavigate={(t) => setTab(t as Tab)}
            />
          )}
          {tab === "config" && <ConfigPage />}
          {tab === "rules" && <RulesPage />}
          {tab === "addons" && <AddonsPage />}
          {tab === "memory" && (
            <MemoryPage status={memory} onStatusChange={setMemory} />
          )}
        </main>
      </div>
    </div>
  );
}
