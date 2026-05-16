import { useEffect, useState } from "react";
import { ChevronsUpDown, Folder, Plus, Check, AlertCircle } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import { toast } from "sonner";
import { api, type ProjectEntry, type ProjectSummary } from "@/api";
import { cn } from "@/lib/utils";

type Props = {
  current: ProjectSummary | null;
  onSwitched: (next: ProjectSummary) => void;
};

function shortPath(p: string): string {
  return p.replace(/^\/Users\/[^/]+/, "~");
}

function formatRelative(epoch: number): string {
  if (!epoch) return "—";
  const delta = Math.max(0, Math.floor(Date.now() / 1000) - epoch);
  if (delta < 60) return `${delta}s ago`;
  if (delta < 3600) return `${Math.floor(delta / 60)}m ago`;
  if (delta < 86400) return `${Math.floor(delta / 3600)}h ago`;
  return `${Math.floor(delta / 86400)}d ago`;
}

export function ProjectSwitcher({ current, onSwitched }: Props) {
  const [open, setOpen] = useState(false);
  const [projects, setProjects] = useState<ProjectEntry[] | null>(null);
  const [orphanSlugs, setOrphanSlugs] = useState<string[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [newPath, setNewPath] = useState("");
  const [adding, setAdding] = useState(false);

  const refresh = async () => {
    try {
      const r = await api.projectsList();
      setProjects(r.projects);
      setOrphanSlugs(r.orphan_slugs);
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  useEffect(() => {
    if (open) refresh();
  }, [open]);

  const switchTo = async (p: { slug?: string; path?: string }) => {
    setBusy(p.slug ?? p.path ?? "");
    try {
      const res = await api.projectsSwitch(p);
      onSwitched(res.current);
      toast.success(`Switched to ${res.current.name}`);
      setOpen(false);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  const addNew = async () => {
    const path = newPath.trim();
    if (!path) {
      toast.error("Path is empty");
      return;
    }
    setAdding(true);
    try {
      await switchTo({ path });
      setNewPath("");
    } finally {
      setAdding(false);
    }
  };

  const removeProject = async (slug: string) => {
    setBusy(slug);
    try {
      await api.projectsRemove(slug);
      await refresh();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  return (
    <Dialog open={open} onOpenChange={setOpen}>
      <DialogTrigger asChild>
        <button
          type="button"
          className="hover:bg-sidebar-accent/60 group flex w-full items-center gap-2 rounded-md px-2 py-2 text-left transition-colors"
        >
          <div className="grid size-8 place-items-center rounded-md bg-violet-500/15 text-violet-500 ring-1 ring-violet-500/30">
            <Folder className="size-4" />
          </div>
          <div className="min-w-0 flex-1">
            <div className="truncate text-sm font-medium">
              {current?.name ?? "—"}
            </div>
            <div className="text-muted-foreground truncate font-mono text-[10px]">
              {current ? shortPath(current.path) : "no project"}
            </div>
          </div>
          <ChevronsUpDown className="text-muted-foreground size-3.5 shrink-0" />
        </button>
      </DialogTrigger>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle>Switch project</DialogTitle>
          <DialogDescription>
            All registered projects on this machine. Click to switch — the
            UI keeps the same URL/token.
          </DialogDescription>
        </DialogHeader>

        <div className="max-h-[420px] space-y-1.5 overflow-y-auto">
          {projects === null ? (
            <div className="text-muted-foreground py-8 text-center text-sm">
              Loading…
            </div>
          ) : projects.length === 0 ? (
            <div className="text-muted-foreground py-8 text-center text-sm">
              No projects registered yet.
            </div>
          ) : (
            projects.map((p) => {
              const isCurrent = current?.slug === p.slug;
              return (
                <div
                  key={p.slug}
                  className={cn(
                    "group flex items-center gap-3 rounded-md border px-3 py-2.5 transition-colors",
                    isCurrent
                      ? "bg-violet-500/5 ring-1 ring-violet-500/40"
                      : "hover:bg-muted/50",
                    !p.exists && "opacity-60",
                  )}
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex items-center gap-2">
                      <div className="truncate text-sm font-medium">
                        {p.name}
                      </div>
                      {isCurrent && (
                        <Badge variant="secondary" className="gap-1 text-[10px]">
                          <Check className="size-3" />
                          active
                        </Badge>
                      )}
                      {!p.exists && (
                        <Badge variant="destructive" className="gap-1 text-[10px]">
                          <AlertCircle className="size-3" />
                          missing
                        </Badge>
                      )}
                    </div>
                    <div className="text-muted-foreground truncate font-mono text-[11px]">
                      {shortPath(p.path)}
                    </div>
                    <div className="text-muted-foreground mt-0.5 flex gap-3 text-[10px] uppercase tracking-wider">
                      <span>{p.slug}</span>
                      <span>last used {formatRelative(p.last_used_at)}</span>
                    </div>
                  </div>
                  <div className="flex shrink-0 items-center gap-2">
                    {!isCurrent && p.exists && (
                      <Button
                        variant="secondary"
                        size="sm"
                        disabled={busy !== null}
                        onClick={() => switchTo({ slug: p.slug })}
                      >
                        {busy === p.slug ? "…" : "Switch"}
                      </Button>
                    )}
                    {!isCurrent && (
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={busy !== null}
                        onClick={() => removeProject(p.slug)}
                      >
                        Remove
                      </Button>
                    )}
                  </div>
                </div>
              );
            })
          )}
        </div>

        {orphanSlugs.length > 0 && (
          <>
            <Separator />
            <div className="space-y-1">
              <div className="text-muted-foreground text-[10px] font-semibold uppercase tracking-wider">
                Orphan slugs
              </div>
              <p className="text-muted-foreground text-xs">
                Memory data exists but the original abs path is unknown.
                Re-add by pointing at the folder below.
              </p>
              <div className="text-muted-foreground flex flex-wrap gap-1.5 pt-1 font-mono text-[11px]">
                {orphanSlugs.map((s) => (
                  <span
                    key={s}
                    className="bg-muted rounded px-2 py-0.5"
                  >
                    {s}
                  </span>
                ))}
              </div>
            </div>
          </>
        )}

        <Separator />
        <div className="space-y-2">
          <Label className="text-[10px] font-semibold uppercase tracking-wider">
            Add new project
          </Label>
          <div className="flex gap-2">
            <Input
              placeholder="/abs/path/to/project"
              value={newPath}
              onChange={(e) => setNewPath(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") addNew();
              }}
              className="font-mono text-xs"
            />
            <Button onClick={addNew} disabled={adding || !newPath.trim()}>
              <Plus className="mr-1 size-3.5" />
              Add &amp; switch
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
