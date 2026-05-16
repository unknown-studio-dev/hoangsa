import type { LucideIcon } from "lucide-react";
import { Card, CardContent } from "@/components/ui/card";
import { cn } from "@/lib/utils";

export type StatTone = "emerald" | "sky" | "violet" | "amber" | "rose";

const TONES: Record<StatTone, string> = {
  emerald: "bg-emerald-500/10 text-emerald-500 ring-emerald-500/20",
  sky: "bg-sky-500/10 text-sky-500 ring-sky-500/20",
  violet: "bg-violet-500/10 text-violet-500 ring-violet-500/20",
  amber: "bg-amber-500/10 text-amber-500 ring-amber-500/20",
  rose: "bg-rose-500/10 text-rose-500 ring-rose-500/20",
};

export type StatItem = {
  icon: LucideIcon;
  label: string;
  value: string;
  delta?: string;
  tone: StatTone;
  onClick?: () => void;
};

export function StatsGrid({ items }: { items: StatItem[] }) {
  return (
    <div className="grid grid-cols-1 gap-3 sm:grid-cols-2 lg:grid-cols-4">
      {items.map(({ icon: Icon, label, value, delta, tone, onClick }) => (
        <Card
          key={label}
          className={cn(
            "border-border/60 py-0",
            onClick && "hover:border-foreground/20 cursor-pointer transition-colors",
          )}
          onClick={onClick}
        >
          <CardContent className="flex items-center gap-3 p-4">
            <div
              className={cn(
                "grid size-10 place-items-center rounded-lg ring-1",
                TONES[tone],
              )}
            >
              <Icon className="size-5" />
            </div>
            <div className="min-w-0">
              <div className="text-muted-foreground text-[10px] uppercase tracking-wider">
                {label}
              </div>
              <div className="truncate text-xl font-semibold tabular-nums">
                {value}
              </div>
              {delta && (
                <div className="text-muted-foreground text-[11px]">{delta}</div>
              )}
            </div>
          </CardContent>
        </Card>
      ))}
    </div>
  );
}
