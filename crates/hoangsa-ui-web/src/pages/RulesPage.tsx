import { useEffect, useState } from "react";
import { useForm, useFieldArray, Controller } from "react-hook-form";
import { zodResolver } from "@hookform/resolvers/zod";
import { z } from "zod";
import { toast } from "sonner";
import {
  Plus,
  Trash2,
  RefreshCw,
  Download,
  AlertTriangle,
  Ban,
  Inbox,
  Shield,
} from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import { Badge } from "@/components/ui/badge";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter as DF,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { api, type Rule, type RulesListRes } from "@/api";

const DialogFooter = DF;

const ENFORCEMENT_LABELS: Record<string, string> = {
  hook: "Hook",
  preflight: "Preflight",
  prompt: "Prompt",
};

const conditionSchema = z.object({
  field: z.string().min(1, "bắt buộc"),
  op: z.enum(["glob", "regex", "contains", "not_contains", "starts_with"]),
  value: z.string().min(1, "bắt buộc"),
});

const ruleSchema = z.object({
  id: z
    .string()
    .min(1, "ID bắt buộc")
    .regex(/^[a-z0-9-]+$/i, "chỉ gồm chữ cái, số và dấu -"),
  name: z.string().min(1, "Tên bắt buộc"),
  enabled: z.boolean(),
  enforcement: z.enum(["hook", "preflight", "prompt"]),
  matcher: z.string().min(1, "Matcher bắt buộc"),
  conditions: z.array(conditionSchema),
  action: z.enum(["block", "warn"]),
  message: z.string().min(1, "Thông báo bắt buộc"),
});

type RuleForm = z.infer<typeof ruleSchema>;

export function RulesPage() {
  const [data, setData] = useState<RulesListRes | null>(null);
  const [showAdd, setShowAdd] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);

  const refresh = async () => {
    try {
      const r = await api.rulesList();
      setData(r);
    } catch (e) {
      toast.error("Không tải được rules: " + (e as Error).message);
    }
  };

  useEffect(() => {
    refresh();
  }, []);

  const onToggle = async (id: string, enabled: boolean) => {
    try {
      const r = await api.rulesToggle(id, enabled, data?.mtime_ms ?? undefined);
      setData((p) => (p ? { ...p, rules: r.rules, mtime_ms: r.mtime_ms } : p));
      toast.success(enabled ? `Đã bật ${id}` : `Đã tắt ${id}`);
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  const onDelete = async (id: string) => {
    setPendingDelete(null);
    try {
      const r = await api.rulesRemove(id, data?.mtime_ms ?? undefined);
      setData((p) => (p ? { ...p, rules: r.rules, mtime_ms: r.mtime_ms } : p));
      toast.success(`Đã xoá ${id}`);
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  const onSync = async () => {
    try {
      const r = await api.rulesSyncDefaults(data?.mtime_ms ?? undefined);
      toast.success(
        `Synced. Replaced ${r.replaced.length}, added ${r.added.length}, kept ${r.user_kept.length} user`,
      );
      refresh();
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  if (!data) return <Skeleton />;

  return (
    <div className="space-y-4">
      <div className="flex items-center justify-between gap-4">
        <div>
          <p className="text-muted-foreground text-sm">
            <span className="text-foreground font-medium tabular-nums">
              {data.enabled}
            </span>{" "}
            đang bật / {data.count} tổng cộng. Quy tắc chạy trước mỗi tool call để
            tránh thao tác nguy hiểm.
          </p>
        </div>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={refresh}>
            <RefreshCw className="size-3.5" />
            Reload
          </Button>
          <Button variant="outline" size="sm" onClick={onSync}>
            <Download className="size-3.5" />
            Sync defaults
          </Button>
          <Button size="sm" onClick={() => setShowAdd(true)}>
            <Plus className="size-3.5" />
            New rule
          </Button>
        </div>
      </div>

      {data.rules.length === 0 ? (
        <Card>
          <CardContent className="py-16 text-center">
            <div className="bg-muted mx-auto mb-3 grid size-12 place-items-center rounded-full">
              <Inbox className="text-muted-foreground size-5" />
            </div>
            <div className="font-medium">Chưa có rule nào</div>
            <p className="text-muted-foreground mx-auto mt-1 max-w-md text-sm">
              Chạy <code className="bg-muted rounded px-1">hoangsa-cli rule init</code>{" "}
              để khởi tạo bộ default, hoặc thêm rule mới.
            </p>
            <Button className="mt-4" onClick={() => setShowAdd(true)}>
              <Plus className="size-4" />
              Tạo rule đầu tiên
            </Button>
          </CardContent>
        </Card>
      ) : (
        <div className="space-y-2">
          {data.rules.map((r) => (
            <RuleCard
              key={r.id}
              rule={r}
              onToggle={onToggle}
              onDelete={() => setPendingDelete(r.id)}
            />
          ))}
        </div>
      )}

      <Dialog open={showAdd} onOpenChange={setShowAdd}>
        <DialogContent className="max-w-2xl">
          <DialogHeader>
            <DialogTitle>Thêm rule mới</DialogTitle>
            <DialogDescription>
              Rule chạy trước mỗi tool call. Mọi điều kiện đều phải khớp thì rule
              mới kích hoạt.
            </DialogDescription>
          </DialogHeader>
          <RuleForm
            mtime={data.mtime_ms ?? undefined}
            onSaved={() => {
              setShowAdd(false);
              refresh();
            }}
            onCancel={() => setShowAdd(false)}
          />
        </DialogContent>
      </Dialog>

      <Dialog
        open={pendingDelete !== null}
        onOpenChange={(o) => !o && setPendingDelete(null)}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Xoá rule?</DialogTitle>
            <DialogDescription>
              Sẽ gỡ <code className="bg-muted rounded px-1">{pendingDelete}</code>{" "}
              khỏi <code>.hoangsa/rules.json</code>. Có thể khôi phục bằng{" "}
              <em>Sync defaults</em> nếu là rule mặc định.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setPendingDelete(null)}>
              Huỷ
            </Button>
            <Button
              variant="destructive"
              onClick={() => pendingDelete && onDelete(pendingDelete)}
            >
              Xoá
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function RuleCard({
  rule,
  onToggle,
  onDelete,
}: {
  rule: Rule;
  onToggle: (id: string, enabled: boolean) => void;
  onDelete: () => void;
}) {
  return (
    <Card className="border-border/60 py-0">
      <CardContent className="p-4">
        <div className="flex items-start gap-3">
          <Switch
            checked={rule.enabled}
            onCheckedChange={(v) => onToggle(rule.id, v)}
            className="mt-1"
          />
          <div className="min-w-0 flex-1">
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-sm font-medium">{rule.name}</span>
              <ActionBadge action={rule.action} />
              <Badge variant="outline" className="text-[10px] uppercase tracking-wider">
                {ENFORCEMENT_LABELS[rule.enforcement] ?? rule.enforcement}
              </Badge>
              {rule.stateful && (
                <Badge variant="secondary" className="text-[10px] uppercase tracking-wider">
                  stateful
                </Badge>
              )}
            </div>
            <div className="text-muted-foreground mt-1 font-mono text-[11px]">
              {rule.id}
            </div>
            <p className="text-muted-foreground mt-2 text-sm">{rule.message}</p>
            <div className="text-muted-foreground mt-2 flex flex-wrap gap-3 text-[11px]">
              <span>
                <span className="text-muted-foreground/70 uppercase tracking-wider">
                  tools
                </span>{" "}
                <code>{rule.matcher}</code>
              </span>
              {rule.conditions.length > 0 && (
                <span>
                  <span className="text-muted-foreground/70 uppercase tracking-wider">
                    conditions
                  </span>{" "}
                  <span className="tabular-nums">{rule.conditions.length}</span>
                </span>
              )}
            </div>
          </div>
          <Button
            variant="ghost"
            size="icon"
            onClick={onDelete}
            className="text-muted-foreground hover:text-rose-500"
          >
            <Trash2 className="size-4" />
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function ActionBadge({ action }: { action: string }) {
  if (action === "block") {
    return (
      <Badge className="border-rose-500/20 bg-rose-500/10 text-rose-500 hover:bg-rose-500/10 gap-1">
        <Ban className="size-3" />
        Chặn
      </Badge>
    );
  }
  return (
    <Badge className="border-amber-500/20 bg-amber-500/10 text-amber-600 hover:bg-amber-500/10 gap-1">
      <AlertTriangle className="size-3" />
      Cảnh báo
    </Badge>
  );
}

function Skeleton() {
  return (
    <div className="space-y-3">
      <div className="bg-muted h-12 animate-pulse rounded" />
      <div className="bg-muted h-24 animate-pulse rounded" />
      <div className="bg-muted h-24 animate-pulse rounded" />
    </div>
  );
}

function RuleForm({
  mtime,
  onSaved,
  onCancel,
}: {
  mtime?: number;
  onSaved: () => void;
  onCancel: () => void;
}) {
  const {
    register,
    control,
    handleSubmit,
    formState: { errors, isSubmitting },
  } = useForm<RuleForm>({
    resolver: zodResolver(ruleSchema),
    defaultValues: {
      id: "",
      name: "",
      enabled: true,
      enforcement: "prompt",
      matcher: "Edit|Write",
      conditions: [{ field: "file_path", op: "contains", value: "" }],
      action: "warn",
      message: "",
    },
  });
  const { fields, append, remove } = useFieldArray({
    control,
    name: "conditions",
  });

  const onSubmit = async (rule: RuleForm) => {
    try {
      await api.rulesAdd({ rule: rule as Rule, expected_mtime_ms: mtime });
      toast.success(`Đã thêm ${rule.id}`);
      onSaved();
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  return (
    <form onSubmit={handleSubmit(onSubmit)} className="space-y-4">
      <div className="grid grid-cols-2 gap-3">
        <div>
          <Label htmlFor="rule-id">ID</Label>
          <Input
            id="rule-id"
            placeholder="vd: no-secrets-in-config"
            {...register("id")}
            className="mt-1.5 font-mono"
          />
          {errors.id && (
            <p className="text-destructive mt-1 text-xs">{errors.id.message}</p>
          )}
        </div>
        <div>
          <Label htmlFor="rule-name">Tên hiển thị</Label>
          <Input
            id="rule-name"
            placeholder="Chặn lưu secret vào config"
            {...register("name")}
            className="mt-1.5"
          />
          {errors.name && (
            <p className="text-destructive mt-1 text-xs">
              {errors.name.message}
            </p>
          )}
        </div>
      </div>

      <div className="grid grid-cols-3 gap-3">
        <div>
          <Label>Tool matcher (regex)</Label>
          <Input
            placeholder="Edit|Write"
            {...register("matcher")}
            className="mt-1.5 font-mono"
          />
        </div>
        <div>
          <Label>Mức enforcement</Label>
          <Controller
            control={control}
            name="enforcement"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger className="mt-1.5">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="prompt">Prompt — hỏi user</SelectItem>
                  <SelectItem value="hook">Hook — chặn ngay</SelectItem>
                  <SelectItem value="preflight">Preflight</SelectItem>
                </SelectContent>
              </Select>
            )}
          />
        </div>
        <div>
          <Label>Hành động</Label>
          <Controller
            control={control}
            name="action"
            render={({ field }) => (
              <Select value={field.value} onValueChange={field.onChange}>
                <SelectTrigger className="mt-1.5">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="warn">Cảnh báo</SelectItem>
                  <SelectItem value="block">Chặn</SelectItem>
                </SelectContent>
              </Select>
            )}
          />
        </div>
      </div>

      <div>
        <Label htmlFor="rule-message">Thông báo cho user</Label>
        <Input
          id="rule-message"
          placeholder="Không lưu API key vào config — đặt vào .env"
          {...register("message")}
          className="mt-1.5"
        />
        {errors.message && (
          <p className="text-destructive mt-1 text-xs">
            {errors.message.message}
          </p>
        )}
      </div>

      <div>
        <div className="mb-2 flex items-center justify-between">
          <Label>Conditions (tất cả phải khớp)</Label>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            onClick={() =>
              append({ field: "file_path", op: "contains", value: "" })
            }
          >
            <Plus className="size-3.5" />
            Add
          </Button>
        </div>
        <div className="space-y-2">
          {fields.map((f, idx) => (
            <div
              key={f.id}
              className="grid grid-cols-[1fr_140px_1fr_auto] gap-2"
            >
              <Input
                placeholder="field"
                {...register(`conditions.${idx}.field`)}
                className="font-mono text-sm"
              />
              <Controller
                control={control}
                name={`conditions.${idx}.op`}
                render={({ field }) => (
                  <Select value={field.value} onValueChange={field.onChange}>
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="contains">contains</SelectItem>
                      <SelectItem value="not_contains">not_contains</SelectItem>
                      <SelectItem value="starts_with">starts_with</SelectItem>
                      <SelectItem value="glob">glob</SelectItem>
                      <SelectItem value="regex">regex</SelectItem>
                    </SelectContent>
                  </Select>
                )}
              />
              <Input
                placeholder="value"
                {...register(`conditions.${idx}.value`)}
                className="font-mono text-sm"
              />
              <Button
                type="button"
                variant="ghost"
                size="icon"
                onClick={() => remove(idx)}
                disabled={fields.length === 1}
              >
                <Trash2 className="size-4" />
              </Button>
            </div>
          ))}
        </div>
      </div>

      <DialogFooter className="pt-2">
        <Button type="button" variant="outline" onClick={onCancel}>
          Huỷ
        </Button>
        <Button type="submit" disabled={isSubmitting}>
          <Shield className="size-4" />
          Lưu rule
        </Button>
      </DialogFooter>
    </form>
  );
}

