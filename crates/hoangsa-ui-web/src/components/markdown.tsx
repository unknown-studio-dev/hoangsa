import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { cn } from "@/lib/utils";

/// Render trusted markdown coming from the user's own memory files.
/// react-markdown is safe by default (no raw HTML), and we lean on
/// tailwind classes per element rather than the typography plugin to
/// keep the dep surface small.
export function Markdown({
  source,
  className,
}: {
  source: string;
  className?: string;
}) {
  return (
    <div className={cn("text-sm leading-relaxed", className)}>
      <ReactMarkdown
        remarkPlugins={[remarkGfm]}
        components={{
          h1: (p) => (
            <h1 className="mt-4 mb-2 text-base font-semibold" {...p} />
          ),
          h2: (p) => (
            <h2 className="mt-4 mb-2 text-sm font-semibold" {...p} />
          ),
          h3: (p) => (
            <h3
              className="mt-3 mb-1 text-sm font-semibold text-foreground/90"
              {...p}
            />
          ),
          h4: (p) => (
            <h4 className="mt-2 mb-1 text-xs font-semibold uppercase tracking-wider text-muted-foreground" {...p} />
          ),
          p: (p) => <p className="my-2 text-foreground/85" {...p} />,
          ul: (p) => <ul className="my-2 ml-5 list-disc space-y-1" {...p} />,
          ol: (p) => <ol className="my-2 ml-5 list-decimal space-y-1" {...p} />,
          li: (p) => <li className="text-foreground/85" {...p} />,
          a: (p) => (
            <a
              className="text-primary underline underline-offset-2 hover:no-underline"
              target="_blank"
              rel="noreferrer"
              {...p}
            />
          ),
          code: ({ className: cls, children, ...rest }) => {
            const isBlock = cls?.startsWith("language-");
            if (isBlock) {
              return (
                <code className={cn("font-mono text-xs", cls)} {...rest}>
                  {children}
                </code>
              );
            }
            return (
              <code
                className="bg-muted rounded px-1 py-0.5 font-mono text-[0.85em]"
                {...rest}
              >
                {children}
              </code>
            );
          },
          pre: (p) => (
            <pre
              className="bg-muted/60 my-2 overflow-x-auto rounded p-3 text-xs"
              {...p}
            />
          ),
          blockquote: (p) => (
            <blockquote
              className="border-muted-foreground/30 text-muted-foreground my-2 border-l-2 pl-3 italic"
              {...p}
            />
          ),
          hr: () => <hr className="border-border my-3" />,
          table: (p) => (
            <div className="my-2 overflow-x-auto">
              <table className="w-full text-xs" {...p} />
            </div>
          ),
          th: (p) => (
            <th
              className="border-border bg-muted/40 border-b px-2 py-1 text-left font-semibold"
              {...p}
            />
          ),
          td: (p) => (
            <td className="border-border/50 border-b px-2 py-1" {...p} />
          ),
          strong: (p) => <strong className="font-semibold" {...p} />,
          em: (p) => <em className="italic" {...p} />,
        }}
      >
        {source}
      </ReactMarkdown>
    </div>
  );
}
