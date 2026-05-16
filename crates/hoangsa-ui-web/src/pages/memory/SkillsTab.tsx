import { useEffect, useState } from "react";
import { toast } from "sonner";
import { RefreshCw, Wrench } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { api, type SkillEntry } from "@/api";

export function SkillsTab() {
  const [skills, setSkills] = useState<SkillEntry[] | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = async () => {
    setLoading(true);
    try {
      const res = await api.memorySkills();
      setSkills(res.data);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-muted-foreground text-sm">
          Skills cài trong{" "}
          <code className="font-mono">.hoangsa/memory/skills/</code> — agent có
          thể trigger qua matcher. Read-only ở đây.
        </p>
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

      {skills === null && (
        <div className="bg-muted h-32 animate-pulse rounded" />
      )}

      {skills && skills.length === 0 && (
        <Card>
          <CardContent className="text-muted-foreground py-6 text-center text-sm">
            Chưa có skill nào. Drop folder vào{" "}
            <code className="font-mono">.hoangsa/memory/skills/</code> để cài.
          </CardContent>
        </Card>
      )}

      {skills && skills.length > 0 && (
        <div className="grid grid-cols-1 gap-3 md:grid-cols-2">
          {skills.map((s) => (
            <Card key={s.slug}>
              <CardContent className="space-y-1 pt-4">
                <div className="flex items-center gap-2 text-sm font-medium">
                  <Wrench className="text-muted-foreground size-3.5" />
                  <span className="font-mono">{s.slug}</span>
                </div>
                <p className="text-muted-foreground text-xs">{s.description}</p>
              </CardContent>
            </Card>
          ))}
        </div>
      )}
    </div>
  );
}
