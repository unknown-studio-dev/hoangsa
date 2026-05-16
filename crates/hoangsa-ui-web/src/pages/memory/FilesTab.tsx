import { useCallback, useEffect, useState } from "react";
import { toast } from "sonner";
import { RefreshCw, Plus, Trash2, FileText, Code2, Eye } from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Badge } from "@/components/ui/badge";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Markdown } from "@/components/markdown";
import { cn } from "@/lib/utils";
import { api, type MemoryFilesRes, type MemoryHealthRes } from "@/api";

type SectionKey = "user" | "memory" | "lessons";

type Section = {
  key: SectionKey;
  title: string;
  description: string;
  /// kind passed to `memory_remove`.
  removeKind: "fact" | "lesson" | "preference";
};

const SECTIONS: Section[] = [
  {
    key: "memory",
    title: "MEMORY.md",
    description: "Project facts — invariants, decisions, paths.",
    removeKind: "fact",
  },
  {
    key: "lessons",
    title: "LESSONS.md",
    description: "Trigger → advice rules that fire while you work.",
    removeKind: "lesson",
  },
  {
    key: "user",
    title: "USER.md",
    description: "Cross-project preferences (first-person).",
    removeKind: "preference",
  },
];

type LiveData = {
  user: string | null;
  memory: string | null;
  lessons: string | null;
  paths: Record<SectionKey, string>;
};

type AddTarget = SectionKey | null;
type RemoveTarget = { key: SectionKey; kind: Section["removeKind"] } | null;

export function FilesTab({ status }: { status: MemoryHealthRes }) {
  const degraded = !status.ok;
  const [data, setData] = useState<LiveData | null>(null);
  const [loading, setLoading] = useState(false);
  const [showRaw, setShowRaw] = useState(false);
  const [addTarget, setAddTarget] = useState<AddTarget>(null);
  const [removeTarget, setRemoveTarget] = useState<RemoveTarget>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      if (degraded) {
        const res = await api.memoryFiles();
        setData(filesToLive(res));
      } else {
        const res = await api.memoryShow();
        // Need paths too — fall back to the FS-direct read just for those
        // labels. It's a cheap stat-only read on three filenames.
        let paths: Record<SectionKey, string>;
        try {
          const filesRes = await api.memoryFiles();
          paths = pathsFrom(filesRes);
        } catch {
          paths = { user: "USER.md", memory: "MEMORY.md", lessons: "LESSONS.md" };
        }
        setData({
          user: res.data.user_md,
          memory: res.data.memory_md,
          lessons: res.data.lessons_md,
          paths,
        });
      }
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [degraded]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-muted-foreground text-sm">
          3 file markdown trong{" "}
          <code className="font-mono">~/.hoangsa/memory/projects/{status.project_slug}/</code>
          .
        </p>
        <div className="flex gap-2">
          <Button
            variant="outline"
            size="sm"
            onClick={() => setShowRaw((v) => !v)}
            title={showRaw ? "Switch to rendered view" : "Switch to raw source"}
          >
            {showRaw ? (
              <>
                <Eye className="size-3.5" />
                Rendered
              </>
            ) : (
              <>
                <Code2 className="size-3.5" />
                Raw
              </>
            )}
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => void refresh()}
            disabled={loading}
          >
            <RefreshCw className="size-3.5" />
            Reload
          </Button>
        </div>
      </div>

      {degraded && (
        <Alert>
          <FileText className="size-4" />
          <AlertTitle>Degraded mode — read-only</AlertTitle>
          <AlertDescription>
            Daemon down: đọc trực tiếp file. Add / Remove yêu cầu daemon up để
            không lệch embedder index.
          </AlertDescription>
        </Alert>
      )}

      {data === null ? (
        <div className="bg-muted h-32 animate-pulse rounded" />
      ) : (
        SECTIONS.map((s) => (
          <FileSection
            key={s.key}
            section={s}
            body={data[s.key]}
            path={data.paths[s.key]}
            degraded={degraded}
            showRaw={showRaw}
            onAdd={() => setAddTarget(s.key)}
            onRemove={() =>
              setRemoveTarget({ key: s.key, kind: s.removeKind })
            }
          />
        ))
      )}

      <AddDialog
        target={addTarget}
        onClose={() => setAddTarget(null)}
        onDone={() => {
          setAddTarget(null);
          void refresh();
        }}
      />
      <RemoveDialog
        target={removeTarget}
        onClose={() => setRemoveTarget(null)}
        onDone={() => {
          setRemoveTarget(null);
          void refresh();
        }}
      />
    </div>
  );
}

function FileSection({
  section,
  body,
  path,
  degraded,
  showRaw,
  onAdd,
  onRemove,
}: {
  section: Section;
  body: string | null;
  path: string;
  degraded: boolean;
  showRaw: boolean;
  onAdd: () => void;
  onRemove: () => void;
}) {
  const isEmpty = body === null || body.trim().length === 0;
  return (
    <Card>
      <CardHeader className="flex flex-row items-start justify-between gap-3 border-b">
        <div className="space-y-1">
          <CardTitle className="text-sm">
            {section.title}
            {body !== null && (
              <Badge variant="outline" className="ml-2 font-mono">
                {body.length}b
              </Badge>
            )}
          </CardTitle>
          <p className="text-muted-foreground text-xs">{section.description}</p>
          <p className="text-muted-foreground font-mono text-[10px] break-all">
            {path}
          </p>
        </div>
        {!degraded && (
          <div className="flex gap-2">
            <Button variant="outline" size="sm" onClick={onAdd}>
              <Plus className="size-3.5" />
              Add
            </Button>
            <Button
              variant="outline"
              size="sm"
              onClick={onRemove}
              disabled={isEmpty}
            >
              <Trash2 className="size-3.5" />
              Remove
            </Button>
          </div>
        )}
      </CardHeader>
      <CardContent
        className={cn(
          "pt-4",
          !isEmpty && "max-h-[520px] overflow-y-auto",
        )}
      >
        {body === null ? (
          <p className="text-muted-foreground text-xs italic">(file missing)</p>
        ) : body.trim().length === 0 ? (
          <p className="text-muted-foreground text-xs italic">(empty)</p>
        ) : showRaw ? (
          <pre className="bg-muted/40 rounded p-3 text-xs whitespace-pre-wrap">
            {body}
          </pre>
        ) : (
          <Markdown source={body} />
        )}
      </CardContent>
    </Card>
  );
}

function AddDialog({
  target,
  onClose,
  onDone,
}: {
  target: AddTarget;
  onClose: () => void;
  onDone: () => void;
}) {
  const [text, setText] = useState("");
  const [tags, setTags] = useState("");
  const [trigger, setTrigger] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (target) {
      setText("");
      setTags("");
      setTrigger("");
    }
  }, [target]);

  const submit = async () => {
    if (!target) return;
    const tagList = tags
      .split(",")
      .map((t) => t.trim())
      .filter(Boolean);
    setBusy(true);
    try {
      if (target === "memory") {
        if (!text.trim()) throw new Error("text bắt buộc");
        await api.memoryRememberFact({ text: text.trim(), tags: tagList });
        toast.success("Đã thêm vào MEMORY.md");
      } else if (target === "user") {
        if (!text.trim()) throw new Error("text bắt buộc");
        await api.memoryRememberPreference({
          text: text.trim(),
          tags: tagList,
        });
        toast.success("Đã thêm vào USER.md");
      } else {
        if (!trigger.trim() || !text.trim())
          throw new Error("trigger + advice bắt buộc");
        await api.memoryRememberLesson({
          trigger: trigger.trim(),
          advice: text.trim(),
        });
        toast.success("Đã thêm vào LESSONS.md");
      }
      onDone();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={!!target} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>
            Add to{" "}
            {target === "memory"
              ? "MEMORY.md"
              : target === "user"
                ? "USER.md"
                : "LESSONS.md"}
          </DialogTitle>
          <DialogDescription>
            Daemon sẽ append vào file và cập nhật vector index.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-3">
          {target === "lessons" && (
            <div className="space-y-1">
              <Label htmlFor="trigger">Trigger</Label>
              <Input
                id="trigger"
                value={trigger}
                onChange={(e) => setTrigger(e.target.value)}
                placeholder="when editing migrations"
              />
            </div>
          )}
          <div className="space-y-1">
            <Label htmlFor="text">
              {target === "lessons" ? "Advice" : "Text"}
            </Label>
            <textarea
              id="text"
              value={text}
              onChange={(e) => setText(e.target.value)}
              rows={4}
              className="border-input bg-background w-full resize-y rounded-md border px-3 py-2 text-sm"
              placeholder={
                target === "lessons"
                  ? "run sqlx prepare after changing SQL"
                  : "Concise fact / preference…"
              }
            />
          </div>
          {target !== "lessons" && (
            <div className="space-y-1">
              <Label htmlFor="tags">Tags (comma-separated)</Label>
              <Input
                id="tags"
                value={tags}
                onChange={(e) => setTags(e.target.value)}
                placeholder="rust, daemon, fixme"
              />
            </div>
          )}
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            Huỷ
          </Button>
          <Button onClick={submit} disabled={busy}>
            {busy ? "Đang thêm…" : "Add"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function RemoveDialog({
  target,
  onClose,
  onDone,
}: {
  target: RemoveTarget;
  onClose: () => void;
  onDone: () => void;
}) {
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (target) setQuery("");
  }, [target]);

  const submit = async () => {
    if (!target) return;
    const q = query.trim();
    if (!q) {
      toast.error("Nhập fragment định danh entry");
      return;
    }
    setBusy(true);
    try {
      await api.memoryRemove({ kind: target.kind, query: q });
      toast.success("Đã xoá entry");
      onDone();
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog open={!!target} onOpenChange={(o) => !o && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Remove entry</DialogTitle>
          <DialogDescription>
            Daemon match theo fragment (substring). Paste 1 dòng độc nhất của
            entry để khoá đúng mục.
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-1">
          <Label htmlFor="fragment">Fragment</Label>
          <textarea
            id="fragment"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            rows={3}
            className="border-input bg-background w-full resize-y rounded-md border px-3 py-2 text-sm"
            placeholder="Copy ~1 dòng từ entry cần xoá"
          />
        </div>
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={busy}>
            Huỷ
          </Button>
          <Button onClick={submit} disabled={busy} variant="destructive">
            {busy ? "Đang xoá…" : "Remove"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function filesToLive(res: MemoryFilesRes): LiveData {
  return {
    user: res.user.body,
    memory: res.memory.body,
    lessons: res.lessons.body,
    paths: pathsFrom(res),
  };
}

function pathsFrom(res: MemoryFilesRes): Record<SectionKey, string> {
  return {
    user: res.user.path,
    memory: res.memory.path,
    lessons: res.lessons.path,
  };
}
