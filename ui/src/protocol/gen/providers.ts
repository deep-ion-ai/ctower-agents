// Gerado por `cargo test -p ct-server --features ts export_ts`.
import type { ProviderInfo } from "./ProviderInfo";

export const PROVIDER_INFOS: ProviderInfo[] = [
  {
    "kind": "jira",
    "label": "Jira",
    "capabilities": {
      "tracker": true,
      "forge": false
    },
    "default_endpoint": null,
    "token_env": "JIRA_API_TOKEN",
    "credential_kinds": [
      "secret",
      "command",
      "file"
    ],
    "default_connection": null
  },
  {
    "kind": "gitlab",
    "label": "GitLab",
    "capabilities": {
      "tracker": false,
      "forge": true
    },
    "default_endpoint": null,
    "token_env": "GITLAB_TOKEN",
    "credential_kinds": [
      "secret",
      "command",
      "file"
    ],
    "default_connection": null
  },
  {
    "kind": "github",
    "label": "GitHub",
    "capabilities": {
      "tracker": true,
      "forge": true
    },
    "default_endpoint": "https://api.github.com",
    "token_env": "GH_TOKEN",
    "credential_kinds": [
      "secret",
      "cli",
      "file",
      "command",
      "none"
    ],
    "default_connection": "github"
  }
];
