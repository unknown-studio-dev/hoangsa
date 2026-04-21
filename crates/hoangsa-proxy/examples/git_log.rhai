// Keep only the first 20 commits and drop blank lines between them.
proxy::register(#{
    cmd: "git",
    subcmd: "log",
    priority: 100,
    filter: |ctx| {
        let ls = proxy::lines(ctx.stdout);
        let trimmed = proxy::head(ls, 20);
        let clean = proxy::grep_out(trimmed, "^\\s*$");
        #{
            stdout: proxy::join(clean),
        }
    }
});
