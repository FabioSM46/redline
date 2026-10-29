# Redline

Pannello di controllo per **GPU AMD su Linux**. Un'unica app per vedere temperature, clock e
consumi, regolare ventole e potenza, e salvare un replay dell'ultimo minuto di gioco con una
scorciatoia. L'ispirazione è il software che AMD distribuisce solo per Windows; Redline ne tiene
le funzioni che si usano davvero e lascia il resto.

> Progetto indipendente, non affiliato né approvato da AMD.

## Stato

In sviluppo, fase **M0 — Fondamenta** (vedi [roadmap](docs/roadmap.md)). Oggi ci sono:

- lettura dei sensori della GPU, testata su una RX 9070 (RDNA4);
- un client per il demone di LACT e il controllo di GPU Screen Recorder, ancora da verificare
  sulla macchina reale;
- un'app Tauri minima che mostra le metriche in tempo reale.

Non esiste ancora un pacchetto da installare.

## Funzionalità previste

| Funzione | Come |
|---|---|
| Statistiche in tempo reale e grafici | lettura diretta da sysfs (`/sys/class/drm/cardN/device`), senza permessi |
| Overlay in gioco (FPS, frametime, temperature) | [MangoHud](https://github.com/flightlessmango/MangoHud), configurato da Redline |
| Tuning: ventole, limite di potenza, clock, undervolt, profili | demone [LACT](https://github.com/ilya-zlobintsev/LACT) (`lactd`) tramite il suo socket |
| Replay istantaneo e registrazione, con audio su tracce separate | [GPU Screen Recorder](https://git.dec05eba.com/gpu-screen-recorder/about/) come processo esterno, con encoder hardware |

Redline non riscrive questi strumenti: li coordina sotto un'unica interfaccia. Il perché è in
[docs/context.md](docs/context.md), le scelte in [docs/decisions.md](docs/decisions.md).

## Requisiti per l'uso

- Linux con una GPU AMD che usa il driver `amdgpu` (tutte le Radeon recenti)
- Wayland o X11
- Per il tuning: LACT con il demone `lactd` attivo
- Per replay e registrazione: GPU Screen Recorder

Su Ubuntu 24.04 lo script `scripts/install-deps.sh` installa LACT (dal `.deb` ufficiale) e GPU
Screen Recorder (da Flathub):

```bash
sudo bash scripts/install-deps.sh
```

Undervolt e overclock richiedono anche il parametro kernel `amdgpu.ppfeaturemask=0xffffffff`.

## Stack

- **Backend**: Rust (edition 2024), [Tauri 2](https://tauri.app)
- **Frontend**: React 19 + TypeScript, Vite, pnpm
- **Logica di sistema**: crate `redline-core`, senza dipendenze da Tauri

## Struttura del repo

```
redline/
├── src/                    # frontend React + TypeScript
├── src-tauri/              # workspace Rust
│   ├── src/                # app Tauri: comandi esposti al frontend
│   ├── crates/
│   │   ├── redline-core/   # sensori, client di lactd, controllo di GPU Screen Recorder
│   │   └── redline-probe/  # CLI per provare i pezzi di redline-core senza UI
│   └── tauri.conf.json
├── docs/
│   ├── context.md          # perché esiste, cosa prende e cosa lascia
│   ├── decisions.md        # registro delle decisioni
│   ├── architecture.md     # moduli e flussi
│   └── roadmap.md          # milestone
├── scripts/                # install-deps.sh, helper della pipeline e test dell'automazione
├── .github/                # CI, review DeepSeek, labeler, template delle issue
├── .claude/skills/         # skill per gli agenti (/dev-issue, /process-pr, /develop-iteration, /scrum-master)
├── AGENTS.md               # regole per gli agenti AI e pipeline
└── package.json
```

## Sviluppo

### Requisiti

| Strumento | Versione | Serve per |
| ----- | ------------------------------- | ------------------ |
| Rust  | fissata in `src-tauri/rust-toolchain.toml` (rustfmt + clippy) | backend |
| Node  | 22.18 o successivo              | frontend |
| pnpm  | 10                              | frontend |
| gh    | 2.18.0 o successivo             | script della pipeline |
| jq    | qualunque recente               | script della pipeline, `scripts/test/` |
| python3 | 3.12 consigliato              | test del reviewer DeepSeek |

Librerie di sistema per Tauri, su Ubuntu/Debian:

```bash
sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file \
  libxdo-dev libssl-dev librsvg2-dev
```

### Comandi

```bash
pnpm install          # dipendenze del frontend
pnpm tauri dev        # avvia l'app in sviluppo con hot reload
pnpm tauri build      # build di release e pacchetto .deb
```

Il `.deb` finisce in `src-tauri/target/release/bundle/deb/`.

Controlli rapidi:

```bash
pnpm build                                        # type-check e build del frontend
pnpm test                                         # test del frontend (node --test)
cargo test --manifest-path src-tauri/Cargo.toml   # test del workspace Rust
```

### Provare i pezzi senza UI

`redline-probe` usa `redline-core` direttamente, dalla cartella `src-tauri/`:

```bash
cargo run -p redline-probe -- sensors --watch     # sensori, una riga al secondo
cargo run -p redline-probe -- lact stats          # statistiche da lactd
cargo run -p redline-probe -- audio               # sorgenti audio per GPU Screen Recorder
# replay di 60 s, audio del gioco e microfono su due tracce separate
cargo run -p redline-probe -- replay --track default_output --track default_input
cargo run -p redline-probe -- tracks <file>.mkv   # tracce di un file salvato
```

Nel replay: `s` + Invio salva gli ultimi secondi, `r` + Invio avvia o ferma una registrazione,
`q` + Invio esce. I file vanno in `~/Videos/Redline/`.

### Flusso di lavoro

Il branch di integrazione è `develop` (branch predefinito); `main` riceve solo i rilasci. Il
lavoro passa da issue → PR verso `develop` → CI e review automatica DeepSeek → merge. Le regole
per gli agenti AI e i dettagli della pipeline sono in [AGENTS.md](AGENTS.md).

## Limiti noti

- **Scorciatoie globali sotto Wayland**: passano dal portale GlobalShortcuts, che GNOME
  implementa dalla versione 48. Su versioni precedenti si useranno le scorciatoie
  personalizzate del desktop.
- **Cattura dello schermo sotto Wayland**: la prima volta il desktop chiede quale schermo
  condividere; poi la scelta viene ricordata.
- **Tuning**: ogni modifica va confermata, altrimenti LACT torna da solo ai valori precedenti
  dopo qualche secondo. È una protezione, non un difetto.

## Rilascio

Un tag `v<versione>` su un commit di `main` avvia `.github/workflows/release.yml`, che costruisce
il `.deb` e lo allega a una release di GitHub in bozza. La release si pubblica a mano dopo
averla controllata. Il pacchetto contiene `THIRD-PARTY-LICENSES` con le licenze delle crate e
dei pacchetti npm compilati nell'app, in `/usr/share/doc/redline/` accanto a `copyright`.

## Documentazione

- [Contesto](docs/context.md)
- [Decisioni](docs/decisions.md)
- [Architettura](docs/architecture.md)
- [Roadmap](docs/roadmap.md)

## Licenza

[MIT](LICENSE). LACT, GPU Screen Recorder e MangoHud sono progetti separati con le loro licenze;
GPU Screen Recorder (GPL-3.0) viene solo avviato come programma esterno, il suo codice non fa
parte di Redline.
