// Cargo: strip all progress-ish lines and keep errors / warnings only.
proxy::register(#{
    cmd: "cargo",
    subcmd: "build",
    priority: 100,
    filter: |ctx| {
        let err_lines = proxy::lines(ctx.stderr);
        let dropped = proxy::grep_out(err_lines,
            "^\\s*(Compiling|Checking|Downloaded|Downloading|Updating|Blocking|Fresh|Documenting)");
        #{
            stderr: proxy::join(dropped),
        }
    }
});
