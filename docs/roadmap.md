# Roadmap

Le milestone sono `Iteration N` su GitHub; questa è la direzione, non un calendario.

## M0 — Fondamenta (in corso)

- [x] Workspace Rust con `redline-core` e la CLI `redline-probe`
- [x] Lettura dei sensori da sysfs
- [x] Client di `lactd` e controllo di GPU Screen Recorder (da verificare sulla macchina reale)
- [x] App Tauri minima con le metriche in tempo reale
- [x] Pipeline di sviluppo portata da wow-don

## M1 — Prestazioni

- Dashboard con grafici nel tempo (temperatura, clock, potenza, utilizzo)
- Più GPU sulla stessa macchina
- Test di `sensors` su alberi sysfs finti

## M2 — Registrazione

- Replay istantaneo dall'app, con scelta delle tracce audio
- Registrazione avviata e fermata dall'app
- Comandi per le scorciatoie del desktop (D5)
- Notifica con il file salvato

## M3 — Tuning

- Curva della ventola, limite di potenza, profili
- Conferma delle modifiche nella UI
- Configurazione guidata delle dipendenze (D4)

## M4 — Overlay e rilascio

- Overlay in gioco tramite MangoHud
- Primo `.deb` pubblicato
