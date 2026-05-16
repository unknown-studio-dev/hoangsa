import { useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { Search, Loader2 } from "lucide-react";
import { Input } from "@/components/ui/input";
import { Card, CardContent } from "@/components/ui/card";
import { Badge } from "@/components/ui/badge";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { api, type RecallChunk } from "@/api";

type Scope = "curated" | "archive" | "all";

const DEBOUNCE_MS = 300;

export function RecallTab() {
  const [query, setQuery] = useState("");
  const [scope, setScope] = useState<Scope>("curated");
  const [loading, setLoading] = useState(false);
  const [chunks, setChunks] = useState<RecallChunk[] | null>(null);
  const timer = useRef<number | null>(null);
  const lastQuery = useRef<string>("");

  useEffect(() => {
    const trimmed = query.trim();
    if (!trimmed) {
      setChunks(null);
      return;
    }
    if (timer.current) window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      void runRecall(trimmed, scope);
    }, DEBOUNCE_MS);
    return () => {
      if (timer.current) window.clearTimeout(timer.current);
    };
    // Re-trigger on scope changes too — same query, different store mix.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, scope]);

  async function runRecall(text: string, sc: Scope) {
    setLoading(true);
    lastQuery.current = text;
    try {
      const res = await api.memoryRecall({ query: text, top_k: 8, scope: sc });
      // Drop stale responses if the user kept typing.
      if (lastQuery.current === text) {
        setChunks(res.data.chunks);
      }
    } catch (e) {
      toast.error((e as Error).message);
    } finally {
      setLoading(false);
    }
  }

  return (
    <div className="space-y-4">
      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-[280px] flex-1">
          <Search className="text-muted-foreground absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2" />
          <Input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Recall query (BM25 + symbol + vector)…"
            className="pl-8"
          />
        </div>
        <Select value={scope} onValueChange={(v) => setScope(v as Scope)}>
          <SelectTrigger className="w-[160px]">
            <SelectValue />
          </SelectTrigger>
          <SelectContent>
            <SelectItem value="curated">Curated (code + mem)</SelectItem>
            <SelectItem value="archive">Archive only</SelectItem>
            <SelectItem value="all">All</SelectItem>
          </SelectContent>
        </Select>
        {loading && (
          <Loader2 className="text-muted-foreground size-4 animate-spin" />
        )}
      </div>

      {chunks === null && !loading && (
        <p className="text-muted-foreground text-sm">
          Gõ query để tra cứu memory + code graph + archive.
        </p>
      )}

      {chunks && chunks.length === 0 && !loading && (
        <p className="text-muted-foreground text-sm">Không tìm thấy chunk nào.</p>
      )}

      {chunks && chunks.length > 0 && (
        <div className="space-y-2">
          {chunks.map((c) => (
            <ChunkCard key={c.id} chunk={c} />
          ))}
        </div>
      )}
    </div>
  );
}

function ChunkCard({ chunk }: { chunk: RecallChunk }) {
  const [span0, span1] = chunk.span;
  const location =
    span0 > 0 || span1 > 0
      ? `${chunk.path}:${span0}-${span1}`
      : chunk.path;
  const preview = chunk.body && chunk.body.length > 0 ? chunk.body : chunk.preview;

  const copy = () => {
    void navigator.clipboard.writeText(location);
    toast.success("Copied " + location);
  };

  return (
    <Card>
      <CardContent className="space-y-2 pt-4">
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <Badge variant="outline" className="font-mono">
            {chunk.source}
          </Badge>
          <button
            onClick={copy}
            className="hover:text-foreground text-muted-foreground font-mono break-all text-left transition"
            title="Click để copy"
          >
            {location}
          </button>
          {chunk.symbol && (
            <span className="text-muted-foreground font-mono">
              · {chunk.symbol}
            </span>
          )}
          <span className="text-muted-foreground ml-auto tabular-nums">
            score {chunk.score.toFixed(3)}
          </span>
        </div>
        <pre className="bg-muted/40 text-foreground/80 overflow-x-auto rounded p-2 text-xs whitespace-pre-wrap">
          {preview}
        </pre>
      </CardContent>
    </Card>
  );
}
