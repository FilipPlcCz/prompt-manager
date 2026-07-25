# Prompt Manager

Windows aplikace pro správu textových promptů: kompaktní **sidebar** u levého okraje obrazovky (~15 % šířky, always-on-top) pro rychlé kopírování, **plné okno** pro správu promptů a receptů, **plovoucí tlačítko** (malý always-on-top rámeček vlevo nahoře, klik zobrazí/skryje sidebar), **lokální REST API** a **MCP server** pro přístup LLM nástrojů (Claude Code, Claude Desktop…). UI je anglicky (výchozí), česky a německy — přepíná se v Nastavení.

Postaveno na **Tauri v2** (Rust + WebView2). Jádro, REST API i MCP server jsou **bez externích závislostí** (čistý Rust std) a plně pokryté testy; frontend je čisté HTML/JS bez build kroku.

## Stažení

Hotové buildy jsou na stránce [Releases](../../releases/latest):

- `Prompt Manager_<verze>_x64-setup.exe` — instalátor (doporučeno)
- `PromptManager-portable.exe` — samostatné .exe bez instalace

Vyžaduje Windows 10/11 s WebView2 runtime (Windows 11 ho má; instalátor si ho případně stáhne sám).

## Pojmy

- **Prompt** – název + obsah s placeholdery `{{nazev_promenne}}` + přiřazené recepty.
- **Recept** – samostatná pojmenovaná sada proměnných (`nazev → hodnota`). Spravuje se centrálně v sekci *Recepty* a přiřazuje se k libovolnému počtu promptů.
- **Proměnné** – odvozená množina: sjednocení proměnných ze všech receptů. Horní lišta chipů se plní automaticky; kliknutí na chip vloží `{{nazev}}` na pozici kurzoru v obsahu promptu (v sekci Recepty skočí na příslušné pole), přetažení dělá totéž na místo, kam chip pustíte.
- **Kopírování** – v sidebaru má každý prompt rozbalovací výběr receptu; tlačítko Copy zkopíruje text vyplněný zvoleným receptem (nebo raw). **Shift+klik** na Copy přidá prompt do schránky za předchozí — držením Shiftu tak lze naklikat několik promptů za sebe do jednoho vložení.

## Struktura repozitáře

```
pm-core/     jádro: modely, souborové úložiště, render engine (0 závislostí, 34+ testů)
pm-api/      lokální REST API, ručně psaný HTTP server na 127.0.0.1 (0 závislostí, testy)
pm-mcp/      MCP server (stdio, JSON-RPC 2.0) -> binárka prompt-manager-mcp (0 závislostí, testy)
src-tauri/   Tauri aplikace: okna, tray, zkratka, clipboard, watcher, commands
dist/        frontend (jeden soubor index.html, bez build kroku; v prohlížeči běží s mock backendem)
.github/     CI: testy + Windows build (portable .exe + NSIS installer)
```

## Build na Windows

Prerekvizity: [Rust](https://rustup.rs), Node.js (jen kvůli `@tauri-apps/cli`), WebView2 runtime (Windows 11 už má).

```powershell
npm install -g @tauri-apps/cli

# testy jádra
cargo test -p pm-core -p pm-api -p pm-mcp

# MCP sidecar (Tauri ho přibalí k aplikaci)
cargo build -p pm-mcp --release
mkdir src-tauri/binaries -Force
copy target/release/prompt-manager-mcp.exe src-tauri/binaries/prompt-manager-mcp-x86_64-pc-windows-msvc.exe

# vývoj (živé okno)
cd src-tauri
tauri dev

# release build -> portable exe + NSIS installer
tauri build
# vysledky: src-tauri/target/release/prompt-manager.exe (portable)
#           src-tauri/target/release/bundle/nsis/*.exe (installer)
```

CI (`.github/workflows/build.yml`) dělá totéž automaticky – stačí repozitář pushnout na GitHub a artefakty stáhnout z Actions (nebo pushnout tag `v*` pro release).

## Úložiště (přenositelné)

Výchozí umístění `%APPDATA%/PromptManager/library/` (změnitelné v Nastavení – lze dát na OneDrive apod.):

```
library/
├── prompts/*.md        1 prompt = 1 soubor (YAML frontmatter + obsah)
├── recipes/*.yaml      1 recept = 1 soubor
└── order.json          pořadí promptů
```

Zkopírováním složky přenesete vše na jiný počítač. Aplikace hlídá složku (polling ~1,5 s) a změny zvenku se projeví automaticky.

## REST API

`http://127.0.0.1:8737/api/v1/` (port v Nastavení), hlavička `Authorization: Bearer <api_key>` – klíč najdete v Nastavení. Endpointy: `GET/POST /prompts`, `GET/PUT/DELETE /prompts/{id}`, `PUT /prompts/order`, `POST /prompts/{id}/render` (`{"recipe_id": "...", "overrides": {...}}` → `{"text": "...", "missing": [...]}`), `GET/POST /recipes`, `GET/PUT/DELETE /recipes/{id}`, `GET /variables`.

## MCP server (Claude Code / Claude Desktop)

Binárka `prompt-manager-mcp.exe` (stdio). Konfigurační snippet zkopírujete v Nastavení aplikace, nebo ručně:

```json
{
  "mcpServers": {
    "prompt-manager": {
      "command": "C:\\cesta\\k\\prompt-manager-mcp.exe",
      "args": ["--library", "C:\\Users\\...\\AppData\\Roaming\\PromptManager\\library"]
    }
  }
}
```

Tools: `list_prompts`, `get_prompt`, `render_prompt(id|nazev, recipe?)`, `create_prompt`, `update_prompt`, `delete_prompt`, `list_recipes`, `get_recipe`, `create_recipe`, `update_recipe`, `list_variables`. Prompt i recept lze adresovat názvem.

## Ovládání

- Tray ikona: levý klik = zobrazit/skrýt sidebar, pravý klik = menu.
- Globální zkratka `Ctrl+Alt+P` (změna v Nastavení, projeví se po restartu).
- Sidebar: ⋮⋮ úchyt = přetažení pořadí; select pod názvem = recept pro kopírování; ikona ↗ = plné okno; ikona ⊙ = zobrazit/skrýt plovoucí tlačítko.
- Plovoucí tlačítko „PM“ (vlevo nahoře, nad všemi okny): klik = zobrazit/skrýt sidebar.
- Zavření okna aplikaci neukončí (běží v tray); Ukončit je v tray menu.

## Známá omezení v1 (kandidáti na v1.1)

- Změna API portu/zkratky/složky knihovny se projeví až po restartu aplikace.
- Editor obsahu je prostý textarea (placeholdery se zvýrazňují pod ním a v náhledu, ne přímo v textu).
- Export/import výběru promptů zatím jen kopírováním složky `library/`.
- Světlé téma zatím není.
- Windows: první spuštění může vyžadovat instalaci WebView2 runtime (installer to řeší).

## Licence

MIT
