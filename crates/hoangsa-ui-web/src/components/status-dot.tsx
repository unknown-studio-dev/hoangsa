import { cn } from "@/lib/utils";

type Tone = "ok" | "warn" | "error" | "idle";

const TONE_BG: Record<Tone, string> = {
  ok: "bg-emerald-500",
  warn: "bg-amber-500",
  error: "bg-rose-500",
  idle: "bg-muted-foreground/40",
};

const TONE_RING: Record<Tone, string> = {
  ok: "ring-emerald-500/30",
  warn: "ring-amber-500/30",
  error: "ring-rose-500/30",
  idle: "ring-muted-foreground/20",
};

export function StatusDot({
  tone = "idle",
  pulse = false,
  className,
}: {
  tone?: Tone;
  pulse?: boolean;
  className?: string;
}) {
  return (
    <span className={cn("relative inline-flex size-2.5 shrink-0", className)}>
      <span
        className={cn(
          "absolute inset-0 rounded-full ring-4",
          TONE_BG[tone],
          TONE_RING[tone],
          pulse && "animate-ping opacity-75",
        )}
      />
      <span className={cn("relative inline-flex size-2.5 rounded-full", TONE_BG[tone])} />
    </span>
  );
}
