import { useEffect, useState } from "react";
import { toast } from "sonner";
import { RefreshCw, Puzzle, Inbox, Search } from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { api, type AddonsListRes } from "@/api";

export function AddonsPage() {
  const [data, setData] = useState<AddonsListRes | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [filter, setFilter] = useState("");

  const refresh = async () => {
    try {
      setData(await api.addonsList());
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const onToggle = async (name: string) => {
    if (!data) return;
    const isActive = data.active.includes(name);
    const next = isActive
      ? data.active.filter((n) => n !== name)
      : [...data.active, name];
    setBusy(name);
    try {
      await api.configApply({
        layer: "project",
        patch: [{ op: "replace", path: "/codebase/active_addons", value: next }],
      });
      setData({ ...data, active: next });
      toast.success(isActive ? `Đã tắt ${name}` : `Đã bật ${name}`);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(null);
    }
  };

  if (!data) return <div className="bg-muted h-32 animate-pulse rounded" />;

  const filtered = filter
    ? data.available.filter(
        (a) =>
          a.name.toLowerCase().includes(filter.toLowerCase()) ||
          (a.frameworks ?? []).some((f) =>
            f.toLowerCase().includes(filter.toLowerCase()),
          ),
      )
    : data.available;

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-muted-foreground text-sm">
          <span className="text-foreground font-medium tabular-nums">
            {data.active.length}
          </span>{" "}
          đang dùng / {data.available.length} có sẵn. Worker rules theo
          framework — bật cái phù hợp với stack.
        </p>
        <Button variant="outline" size="sm" onClick={refresh}>
          <RefreshCw className="size-3.5" />
          Reload
        </Button>
      </div>

      <div className="relative max-w-sm">
        <Search className="text-muted-foreground absolute left-3 top-1/2 size-3.5 -translate-y-1/2" />
        <Input
          placeholder="Tìm theo tên hoặc framework…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          className="pl-9"
        />
      </div>

      {filtered.length === 0 ? (
        <Card>
          <CardContent className="py-12 text-center">
            <Inbox className="text-muted-foreground mx-auto mb-2 size-8" />
            <div className="text-muted-foreground text-sm">
              Không có addon khớp "{filter}"
            </div>
          </CardContent>
        </Card>
      ) : (
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          {filtered.map((a) => {
            const active = data.active.includes(a.name);
            return (
              <Card
                key={a.name}
                className={
                  active
                    ? "border-violet-500/40 ring-1 ring-violet-500/20"
                    : "border-border/60"
                }
              >
                <CardContent className="p-4">
                  <div className="flex items-start gap-3">
                    <div
                      className={
                        "grid size-10 shrink-0 place-items-center rounded-lg ring-1 " +
                        (active
                          ? "bg-violet-500/10 text-violet-500 ring-violet-500/20"
                          : "bg-muted text-muted-foreground ring-border")
                      }
                    >
                      <Puzzle className="size-5" />
                    </div>
                    <div className="min-w-0 flex-1">
                      <div className="flex items-baseline justify-between gap-2">
                        <span className="truncate text-sm font-medium">
                          {a.name}
                        </span>
                        <Switch
                          checked={active}
                          disabled={busy === a.name}
                          onCheckedChange={() => onToggle(a.name)}
                        />
                      </div>
                      {a.frameworks && a.frameworks.length > 0 && (
                        <div className="mt-2 flex flex-wrap gap-1">
                          {a.frameworks.map((f) => (
                            <Badge
                              key={f}
                              variant="secondary"
                              className="text-[10px] uppercase tracking-wider"
                            >
                              {f}
                            </Badge>
                          ))}
                        </div>
                      )}
                      {a.description && (
                        <p className="text-muted-foreground mt-2 text-sm">
                          {a.description}
                        </p>
                      )}
                    </div>
                  </div>
                </CardContent>
              </Card>
            );
          })}
        </div>
      )}
    </div>
  );
}
