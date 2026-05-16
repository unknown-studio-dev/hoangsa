import { useState } from "react";
import { toast } from "sonner";
import { Search, Loader2 } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import { api, type ArchiveHit } from "@/api";

export function ArchiveTab() {
  const [query, setQuery] = useState("");
  const [topicFilter, setTopicFilter] = useState("");
  const [loading, setLoading] = useState(false);
  const [hits, setHits] = useState<ArchiveHit[] | null>(null);

  async function run() {
    const q = query.trim();
    if (!q) {
      setHits(null);
      return;
    }
    setLoading(true);
    try {
      const res = await api.memoryArchiveSearch({
        query: q,
        top_k: 20,
        topic: topicFilter.trim() || undefined,
      });
      setHits(res.data);
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="space-y-4">
      <form
        onSubmit={(e) => {
          e.preventDefault();
          void run();
        }}
        className="flex flex-wrap items-center gap-2"
      >
        <div className="relative min-w-[280px] flex-1">
          <Search className="text-muted-foreground absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Tìm trong archive (session exchanges)…"
            className="pl-8"
          />
        </div>
        <Input
          value={topicFilter}
          onChange={(e) => setTopicFilter(e.target.value)}
          placeholder="Topic filter (optional)"
          className="w-[180px]"
        />
        <Button type="submit" size="sm" disabled={loading || !query.trim()}>
          {loading && <Loader2 className="size-3.5 animate-spin" />}
          Tìm
        </Button>
      </form>

      {hits === null && (
        <p className="text-muted-foreground text-sm">
          Archive search dùng vector similarity. Topic = filter theo metadata
          `topic` (vd. "refactor", "debug").
        </p>
      )}

      {hits && hits.length === 0 && (
        <p className="text-muted-foreground text-sm">Không có kết quả.</p>
      )}

      {hits && hits.length > 0 && (
        <div className="space-y-2">
          {hits.map((h) => (
            <ArchiveCard key={h.id} hit={h} />
          ))}
        </div>
      )}
    </div>
  );
}

function ArchiveCard({ hit }: { hit: ArchiveHit }) {
  const topic =
    (hit.metadata?.["topic"] as string | undefined) ?? "conversation";
  const project = hit.metadata?.["project"] as string | undefined;
  return (
    <Card>
      <CardContent className="space-y-2 pt-4">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <Badge variant="outline">{topic}</Badge>
          {project && (
            <Badge variant="outline" className="font-mono">
              {project}
            </Badge>
          )}
          <span className="text-muted-foreground ml-auto tabular-nums">
            d={hit.distance.toFixed(3)}
          </span>
        </div>
        {hit.text && (
          <pre className="bg-muted/40 text-foreground/80 max-h-[240px] overflow-y-auto rounded p-2 text-xs whitespace-pre-wrap">
            {hit.text}
          </pre>
        )}
      </CardContent>
    </Card>
  );
}
