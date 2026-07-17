{
  description = "aldrava — the typed knock: comment-command dispatch for GitHub Actions (trust-gated slash commands -> label/workflow_dispatch/repository_dispatch).";

  inputs.substrate.url = "github:pleme-io/substrate";

  outputs = { substrate, ... }: substrate.rust.tool {
    src = ./.;
    member = "aldrava";
  };
}
