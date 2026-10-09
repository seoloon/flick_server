# Checkpoint — 2026-10-08

Projet: le panel Flick pilote Flick Server (4 sous-projets). Voir `docs/superpowers/specs/2026-10-07-module-lifecycle-settings-design.md` (liste en tête).

## État

| # | Sous-projet | État |
|---|---|---|
| 1 | Cœur serveur (settings.json, start/stop/reload, API admin, erreurs explicites) | **Fait, dans `main`** |
| 2 | Panel: modules + réglages, jeton dérivé de `PANEL_PASSWORD` | **Fait, dans `main`** (`7a4799a`, smoke test OK) |
| 3 | Logs (tampon mémoire serveur + page Logs du panel) | **En cours**: tâches 1-3 sur 9 faites |
| 4 | `.env.example` minimal, panel activé par défaut dans compose | **Pas commencé** (pas de spec ni de plan) |

## Git

- Branche courante: `feature/panel-settings` (HEAD `06267f0`). `main` = `7a4799a` (retard de 6 commits: logs).
- La création de la branche `feature/logs` a été refusée par le harness, donc les logs sont committés sur `feature/panel-settings`.
- Pour fusionner sans toucher à ton working tree: `git branch -f main <commit>` (fast-forward, pas de checkout).
- **Modifs à toi, jamais committées par moi** (ne pas les stager): `README.md` (en-tête/tagline), suppression de `docs/assets/flick-wordmark-*.svg`, ajout de `docs/assets/flickserver-wordmark*.svg`. Toujours `git add <chemins explicites>`, jamais `-A`/`-a`.
- Rien n'est poussé sur un remote.

## Sous-projet 3 (logs) en détail

- Spec: `docs/superpowers/specs/2026-10-08-logs-design.md`. Plan: `docs/superpowers/plans/2026-10-08-logs.md` (9 tâches).
- Fait: T1 (`75f534e` niveaux, masquage des secrets, plafonds), T2 (`0853f59` tampon + curseur), T3 (`dd2b7ff` `FLICKSYNC_LOG_BUFFER`, boot seul, défaut 2000, 0..=10000), fix de revue (`06267f0`: fuites `Bearer`, guillemets échappés, scrub quadratique).
- **Re-review de `06267f0` terminée: les 8 constats sont corrigés, aucun nouveau Critical/Important** (sonde + fuzz 400k cas, 0 fuite, 0 panic, 17/17 tests). Lot A (T1-3) **clos**.
- Mineurs reportés du lot A (masquage = défense en profondeur): double échappement Debug laisse fuiter la fin du secret; `Bearer` suivi d'un espace insécable U+00A0 non détecté; `Authorization: Basic …` non couvert; un guillemet juste après `<redacted>` est conservé; un très gros secret précoce (> 4*max octets) masque les champs suivants. À traiter plus tard si on veut durcir.
- Reste: T4 (couche de capture tracing + câblage), T5 (`GET /admin/v1/logs`, `INVALID_QUERY`, docs serveur), T6 (logique panel + tests node), T7 (route proxy `/api/logs`), T8 (page Logs, entrée sidebar, CSS, `panel/README.md`), T9 (test de bout en bout, navigateur).
- Lots prévus: B = T4-5 (modèle capable, c'est de l'intégration), C = T6-8, puis revue finale avec smoke test (comme le sous-projet 2).
- Réserves connues du plan: le masquage est une défense en profondeur (pas de JSON `"token":"x"`, pas de `Cookie:`/`x-api-key:`); les événements du crate `log` peuvent avoir la cible `log`; pas de test sur une query string illisible; reporter la ligne `FLICKSYNC_LOG_BUFFER` dans le nouveau `.env.example` (sous-projet 4).

## Sous-projet 4 (à faire)

- Réduire `.env.example` au noyau: hôte/port, `FLICKSYNC_DATA_DIR`, `FLICKSYNC_PUBLIC_URL`, format des logs, `FLICKSYNC_LOG_BUFFER`, `PANEL_PASSWORD`, `ENABLE_WEB_PANEL`, clés fournies à la main, `SHUTDOWN_GRACE`, `SWEEP_INTERVAL_MS`. Le reste se règle dans le panel (l'environnement reste un repli).
- Panel activé par défaut dans `docker-compose.yml`; supprimer `FLICKSYNC_ADMIN_TOKEN` partout (panel: `admin-token.ts`, `env-check.mjs`, `config.ts`; docs).
- Mettre à jour `README.md` (attention à tes modifs non committées), `panel/README.md`, `docs/deployment.md`, `TECHNICAL.md` (textes « seule la clé de signature est sur le disque »: `TECHNICAL.md:70,369`, `Dockerfile:59`, `docs/deployment.md:62`).
- Rappel: FlickSync est éteint par défaut; le panel l'active (ou `FLICKSYNC_ENABLED=true`).
- Processus: spec, plan, exécution par sous-agents, revue, comme pour 2 et 3.

## Points ouverts hors sous-projets

- Étape Docker manuelle jamais faite (le daemon était arrêté): `docker compose up`, vérifier panel + serveur.
- Mineurs reportés (sous-projet 1): compteurs `participants_active` / `rtt_avg_us` périmés après un stop; travail bloquant dans les handlers admin async (`spawn_blocking`); lecture des settings en un seul instantané; stop pendant une requête (503 vs 500).
- Mineurs reportés (sous-projet 2): pastilles de l'Overview rafraîchies seulement après une action locale; pas de contrôle de révision sur PUT; jeton legacy posé seulement côté panel = tous les appels rejetés (design accepté).
- Le dossier `.superpowers/sdd/` (ledgers, briefs, diffs) est git-ignoré. Ledger des logs: `.superpowers/sdd/2026-10-08-logs/progress.md`.

## Pour reprendre

1. `git status` + `git log --oneline -8` pour retrouver `06267f0`.
2. Relire `.superpowers/sdd/2026-10-08-logs/progress.md`.
3. Lancer le lot B (T4-5), puis C (T6-8), puis la revue finale avec smoke test (T9), via le skill `superpowers:subagent-driven-development`. Le ledger `.superpowers/sdd/2026-10-08-logs/progress.md` dit où reprendre; les briefs `task-N-brief.md` y sont déjà.
4. Fusionner dans `main` (`git branch -f main HEAD`), puis sous-projet 4.
5. À la fin: tests (`cargo test`, `cd panel; npm test; npm run typecheck; npm run build`), puis demander à l'utilisateur fusion/PR/garder.
