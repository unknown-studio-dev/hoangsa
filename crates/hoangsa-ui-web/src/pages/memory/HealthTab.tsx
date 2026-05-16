import { useState } from "react";
import { toast } from "sonner";
import {
  RefreshCw,
  Power,
  Database,
  Clock,
  ShieldCheck,
  Hash,
  FileCode2,
} from "lucide-react";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { StatusDot } from "@/components/status-dot";
import { api, type MemoryHealthRes } from "@/api";

export function HealthTab({
  status,
  onStatusChange,
}: {
  status: MemoryHealthRes;
  onStatusChange: (s: MemoryHealthRes) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [confirmRestart, setConfirmRestart] = useState(false);

  const refresh = async () => {
    try {
      const s = await api.memoryHealth();
      onStatusChange(s);
      toast.success("Đã cập nhật trạng thái");
    } catch (e) {
      toast.error((e as Error).message);
    }
  };

  const onRestart = async () => {
    setConfirmRestart(false);
    setBusy(true);
    try {
      const r = await api.memoryRestart();
      toast.success(r.message);
      setTimeout(refresh, 600);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setBusy(false);
    }
  };

  const tone = status.ok ? "ok" : "warn";

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-muted-foreground text-sm">
          Daemon <code>hoangsa-memory-mcp</code> phục vụ memory_recall,
          memory_impact, memory_remember_*…
        </p>
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={refresh}>
            <RefreshCw className="size-3.5" />
            Reload
          </Button>
          <Button
            variant="outline"
            size="sm"
            onClick={() => setConfirmRestart(true)}
            disabled={busy}
          >
            <Power className="size-3.5" />
            Restart daemon
          </Button>
        </div>
      </div>

      <Card className="overflow-hidden">
        <CardHeader className="border-b">
          <CardTitle className="flex items-center gap-2 text-sm">
            <ShieldCheck className="size-4 text-emerald-500" />
            Daemon health
          </CardTitle>
        </CardHeader>
        <CardContent className="grid grid-cols-2 gap-x-4 gap-y-5 pt-6 sm:grid-cols-4">
          <Field
            icon={<StatusDot tone={tone} className="-ml-0.5" />}
            label="State"
          >
            <span className={status.ok ? "text-foreground" : "text-amber-600"}>
              {status.ok ? "Connectable" : "Unreachable"}
            </span>
          </Field>
          <Field
            icon={<FileCode2 className="text-muted-foreground size-3.5" />}
            label="Socket"
          >
            {status.socket_exists ? "exists" : "absent"}
          </Field>
          <Field
            icon={<Hash className="text-muted-foreground size-3.5" />}
            label="Project slug"
          >
            <span className="font-mono text-xs">{status.project_slug}</span>
          </Field>
          <Field
            icon={<Clock className="text-muted-foreground size-3.5" />}
            label="Mode"
          >
            on-demand
          </Field>
        </CardContent>
      </Card>

      {!status.ok && (
        <Alert>
          <Database className="size-4" />
          <AlertTitle>Memory đang suy giảm</AlertTitle>
          <AlertDescription>
            Recall / Archive / Skills tạm vô hiệu. Files tab vẫn xem được nội dung
            qua read trực tiếp filesystem, nhưng không sửa được.
          </AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader className="border-b">
          <CardTitle className="text-sm">Chi tiết kỹ thuật</CardTitle>
        </CardHeader>
        <CardContent className="space-y-3 pt-5 text-sm">
          <DetailRow label="Socket path" value={status.socket_path} mono />
          <DetailRow label="Project slug" value={status.project_slug} mono />
          <DetailRow
            label="Status code"
            value={status.ok ? "connectable" : "down"}
          />
        </CardContent>
      </Card>

      <Dialog open={confirmRestart} onOpenChange={setConfirmRestart}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Restart daemon?</DialogTitle>
            <DialogDescription>
              Sẽ gửi SIGTERM cho mọi process <code>hoangsa-memory-mcp</code>{" "}
              đang chạy. Claude Code tự khởi động lại lần sau khi cần memory
              tool.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setConfirmRestart(false)}>
              Huỷ
            </Button>
            <Button onClick={onRestart}>Restart</Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
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

function DetailRow({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <div className="flex items-baseline justify-between gap-4">
      <span className="text-muted-foreground text-[10px] uppercase tracking-wider">
        {label}
      </span>
      <span
        className={"text-right break-all " + (mono ? "font-mono text-xs" : "")}
      >
        {value}
      </span>
    </div>
  );
}
