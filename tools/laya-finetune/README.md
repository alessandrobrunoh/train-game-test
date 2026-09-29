# Fine-tuning di Laya per TrainGame

Kit per mettere a punto `laya-multilingual` sulle decisioni degli NPC su una GPU gratuita
(Kaggle o Colab) e rimettere il risultato nel gioco. Contesto e numeri in `docs/laya-brain.md`
(§9 per la valutazione zero-shot, §10 per questa pipeline).

```
cargo run --example export_dataset  →  train/val/test.jsonl     (Mac, pochi secondi)
finetune.py su Kaggle/Colab (T4)    →  laya-traingame-ft/      (GPU, ~1 h, stima)
LAYA_MODEL_DIR=laya-traingame-ft    →  gioco e laya_eval       (Mac, Metal)
```

| File | Cosa fa |
|---|---|
| `finetune.py` | Training (RLCD come il notebook ufficiale), fit delle temperature, valutazione, salvataggio nel layout che `LayaModel::load` legge |
| `laya_finetune.ipynb` | Notebook minimo per Kaggle/Colab che trova i file e lancia lo script |
| `requirements.txt` | `laya` fissato al commit `9d955671` (0.3.21), `transformers`, `safetensors`, `huggingface_hub` |
| `test_finetune_data.py` | Test senza torch: lettura del dataset, temperature, ECE, controllo della checkpoint |
| `sample/` | Un campione di 26 righe (3 semi) per provare lo script e per i test |

## 1. Esportare il dataset (sul Mac)

```bash
cargo run -p sim-laya --release --example export_dataset -- --out laya-dataset
```

Con i default: 40 semi (10–49) × 3 anni di gioco (36 giorni), 300 NPC ciascuno, circa **26 500
righe** in una ventina di secondi (41 MB). Opzioni principali:

| Opzione | Default | |
|---|---|---|
| `--seeds A..B` o `A,B,C` | `10..49` | estremi inclusi; i semi 1–3 di `laya_eval` si saltano (`--allow-eval-seeds`) |
| `--years N` / `--days N` | 3 anni | giorni di gioco registrati per seme, dopo un giorno di rodaggio |
| `--max-rows N` | 20000 | righe della partita al massimo **per tipo**, divise tra i semi |
| `--kinds` | `actions,deliberations` | |
| `--labels greedy\|sample` | `greedy` | etichetta = opzione più probabile dell'insegnante, o un'estrazione dalla sua distribuzione |
| `--top-k K` | 5 | opzioni per le azioni, le migliori per utilità (come `LayaConfig::top_k`) |
| `--teacher-temp T` | 0.05 | temperatura della softmax sulle utilità di `UtilityBrain` |
| `--obvious-per-kind`, `--obvious-delib-per-kind`, `--obvious-repeat` | 4, 4, 2 | casi ovvi per seme e copie nel train |
| `--max-label-share F` | 0.35 | quota massima di una categoria di etichetta tra le azioni ("ozia" altrimenti è metà) |
| `--val F`, `--test F` | 0.1, 0.1 | frazione di **semi** per validazione e test |
| `--delib-rate X` | 3 | moltiplica la frequenza delle deliberazioni |

Cosa contiene:

- **Azioni della partita**: un campione uniforme delle decisioni di `UtilityBrain`. La domanda è
  quella di `LayaBrain` (contesto `npc_context`, "Quale azione sceglie {nome} adesso?", le top-k
  opzioni). L'insegnante è `softmax(utilità / 0.05)`. `UtilityBrain` sceglie il massimo più un
  rumore uniforme `0..0.08`, quindi la softmax è una sua versione "ammorbidita". Se due opzioni
  hanno la stessa descrizione se ne tiene una.
- **Deliberazioni della partita**: coppia, figlio, furto e protesta, con la distribuzione della
  regola del `sim` come insegnante (è esattamente quella con cui il `sim` estrae).
- **Casi ovvi** (`src/eval.rs`, `src/eval/deliberations.rs`) con le risposte giuste. La
  distribuzione è uniforme sulle opzioni giuste. Nel train sono ripetuti con un altro ordine
  delle opzioni.
- **Dopo la raccolta**:
  - si tolgono i duplicati;
  - gli split si fanno **per seme**, così nessun mondo sta in due split;
  - le opzioni si mescolano riga per riga e l'etichetta si rimappa. Senza, la risposta giusta
    sarebbe quasi sempre la prima (le top-k sono ordinate per utilità) e Laya ha un bias di
    posizione.

**Schema di una riga** (`schema: 1`). È il formato dei casi di `LocalLLaMA/typed-decisions`
usato dal notebook ufficiale, cioè `state` + `questions` + `gold` con la distribuzione
dell'insegnante. Ci sono però due differenze:

- i campi sono oggetti JSON e non stringhe;
- i criteri sono una **lista** di descrizioni. È la forma che Laya rende senza lettere, come fa
  `LayaModel` nel gioco (`OptionLabels::Plain`), e le chiavi di `probabilities` sono quindi le
  descrizioni.

```json
{"schema":1,"id":"s10-del-00000","kind":"deliberation","source":"sim","case":"furto","seed":10,"split":"train",
 "state":"Giorno 3 12:00 (giorno). Marco Coppola, 16 anni (giovane), … Potrebbe chiedere aiuto a Tommaso Coppola (padre), che ha 79 gettoni.",
 "questions":{"decision":{"type":"choice","instructions":"Marco Coppola non può permettersi un vestito: che cosa fa?",
   "criteria":["chiede qualche gettone a Tommaso Coppola (padre)","rinuncia per ora e mette da parte i gettoni","ruba un vestito al Mercato «La Bottega» (carrozza 15)"]}},
 "gold":{"decision":{"label":"chiede qualche gettone a Tommaso Coppola (padre)","label_index":0,
   "probabilities":{"chiede qualche gettone a Tommaso Coppola (padre)":0.64145,"rinuncia per ora e mette da parte i gettoni":0.27599,"ruba un vestito al Mercato «La Bottega» (carrozza 15)":0.08256}}},
 "option_tags":["AskForHelp","Save","Steal"],"label_tag":"AskForHelp"}
```

`kind` vale `action` o `deliberation`, `source` vale `sim` o `obvious`, `case` è "partita", il
tipo di deliberazione o il caso ovvio. `option_tags` dà la categoria di ogni opzione ("mangia",
"Steal"…) e serve per le statistiche. `manifest.json` riassume parametri, semi per split e
conteggi. Il test `crates/sim-laya/tests/dataset.rs` controlla lo schema.

Prima di caricare il dataset si può controllarlo senza GPU (serve solo Python 3):

```bash
python3 tools/laya-finetune/finetune.py --data laya-dataset --check-data
```

## 2. Preparare il pacchetto

```bash
cp tools/laya-finetune/{finetune.py,requirements.txt,laya_finetune.ipynb} laya-dataset/
(cd laya-dataset && zip -r ../laya-dataset.zip .)
```

## 3a. Kaggle (consigliato)

1. Crea un account su kaggle.com e **verifica il telefono** (Settings → Phone verification),
   altrimenti GPU e Internet non sono disponibili.
2. **Datasets → New Dataset**: carica `laya-dataset.zip` (Kaggle lo scompatta) e chiamalo, per
   esempio, `traingame-laya`. Può restare privato.
3. **Code → New Notebook → File → Import Notebook**: scegli `laya_finetune.ipynb`.
4. Pannello a destra, **Settings**:
   - **Accelerator: GPU T4 x2**. Lo script ne usa una sola; va bene anche **GPU P100**.
   - **Internet: On**, per `pip` e per scaricare la base da Hugging Face (~650 MB).
5. **Add Input** → il dataset `traingame-laya`.
6. **Run All**, oppure *Save Version → Save & Run All* per farlo girare anche a browser chiuso
   (massimo 12 h per sessione). La cella 4 (`--check-data`) stampa i conteggi; la cella 5 allena.
7. A fine corsa, nel pannello **Output** (o nella scheda Output della versione salvata) c'è
   `laya-traingame-ft.zip` (~600 MB): scaricalo.

## 3b. Colab

1. colab.research.google.com → **File → Upload notebook** → `laya_finetune.ipynb`.
2. **Runtime → Change runtime type → T4 GPU**.
3. Carica `laya-dataset.zip` dal pannello **File** (o da Google Drive) e scompattalo in una cella:
   `!unzip -q /content/laya-dataset.zip -d /content/laya-dataset`.
4. **Runtime → Run all**. Il notebook trova da solo `train.jsonl` sotto `/content`.
5. Scarica `/content/laya-traingame-ft.zip` dal pannello File, o copialo su Drive
   (`!cp /content/laya-traingame-ft.zip /content/drive/MyDrive/`). Colab gratuito può chiudere
   la sessione dopo qualche ora o se resti inattivo, quindi non lasciarlo in background.

## Tempi e costi attesi

Le stime sono segnate **[I]**: finora lo script non è mai stato eseguito su GPU.

- **Costo:** zero.
  - Kaggle dà circa 30 h di GPU a settimana.
  - Colab gratuito dà una T4 a sessioni di qualche ora, senza garanzie.
- **Riferimento upstream [V]:** il notebook ufficiale impiega circa 4–5 h per 4 epoche su circa
  30k domande. Usa 2×T4 e il modello inglese *large* (ModernBERT-large, `max_len` 1024).
- **Il nostro caso [I]:**
  - mmBERT-base richiede circa 3× meno calcolo per token;
  - le righe sono più corte (circa 270 token stimati, mediana e p95 veri stampati dallo script);
  - le righe sono circa 26k per 3 epoche, su **una** T4.
  - Stima: **circa 45–90 min** in modalità `full` e **circa 20–40 min** con `--mode head`.
  - Lo script stampa una stima dei minuti rimasti dopo 20 lotti.
- **Memoria:** con i default (micro-batch 8, fp16) dovrebbe stare nei 16 GB di una T4. In caso
  di out-of-memory usa `--micro-batch 4 --grad-accum 8 --grad-ckpt`.
- **Disco:** la base occupa ~650 MB. La checkpoint più il suo zip ~1.3 GB, dentro i 20 GB di
  `/kaggle/working`.

## Cosa fa `finetune.py`

1. **Legge e valida** `train/val/test.jsonl` e controlla che nessuna riga stia in due split.
   Accetta anche casi `typed-decisions` con le colonne come stringhe JSON e i criteri
   `{"A": …}`.
2. **Scarica la base** `convaiinnovations/laya-multilingual` al commit fissato
   `e4e9ddf21a7b1903b7acffd8814ad4307bf63a67`, lo stesso di `download.rs`. Poi valuta la base
   zero-shot sul test, per il confronto.
3. **Tokenizza** con `laya.common.build_sequence`, la stessa funzione che il port Rust riproduce
   (`max_len` 384 come `LayaOptions::max_len`, `head_max_len` 256 della checkpoint).
4. **Allena** con la ricetta **RLCD** del notebook ufficiale (`train_ddp.py`), su una GPU:
   - soft cross-entropy sulla distribuzione dell'insegnante;
   - policy gradient su 4 campioni di logit rumorosi (rumore da 0.4 a 0.1), con ricompensa
     `proper_reward` (sferica 0.75, RPS 1.0);
   - AdamW, learning rate 2.5e-5 per l'encoder e 1e-4 per la testa, cosine, fp16 o bf16,
     clip 1.0;
   - batch effettivo 32 (8 × 4);
   - si tiene l'epoca con la cross-entropy di validazione più bassa.

   Varianti:
   - `--mode head` congela l'encoder e allena solo la testa (come stuntd): più veloce, di
     solito meno preciso;
   - `--targets label` allena sull'etichetta one-hot invece che sulla distribuzione;
   - `--rl-weight 0` usa solo la cross-entropy.
5. **Fitta le temperature** sulla validazione, sulle righe mai viste in training (se `val.jsonl`
   manca, sul 10% del train tenuto da parte come fa il notebook):
   - una temperatura per il tipo `choice` e una per bucket di opzioni (`choice:2`,
     `choice:3-5`, che sono le chiavi che il runtime legge);
   - obiettivo: la cross-entropy con la distribuzione dell'insegnante;
   - le temperature restano limitate a [0.5, 5] come in Laya e in Rust;
   - le temperature ereditate per bucket si sostituiscono, come raccomanda upstream.

   Si stampa anche una temperatura per tipo di riga (azione/deliberazione), solo come
   informazione: il runtime non distingue i due tipi.
6. **Valuta sul test**, prima e dopo, con le temperature fittate:
   - misure: accuratezza sull'etichetta, accuratezza "morbida" (Σ p·insegnante), confidenza
     media, ECE (15 fasce, come `laya.common.ece_score`), NLL e Brier;
   - gruppi: in totale, per tipo, per origine, per caso ovvio e per posizione dell'etichetta
     (per vedere il bias di posizione).
7. **Salva**:
   - `model.safetensors` in F16, come upstream;
   - `encoder/config.json` e `tokenizer/`, copiati dalla base perché non cambiano;
   - `rl_agent_config.json` con le temperature nuove;
   - `eval_report.json` e `traingame_finetune.json`.

   Poi controlla il layout (`--check-checkpoint`: gli stessi file di `CheckpointFiles`, gli
   stessi tensori della base), la ricarica con `laya.load` per un controllo di parità e crea lo
   zip. `--push-to-hub utente/repo` la carica privata su Hugging Face (serve `HF_TOKEN`).

Valutare una checkpoint esistente sul test: `python finetune.py --data DIR --eval-only CHECKPOINT`.

## 4. Riportarla nel gioco (sul Mac)

```bash
unzip laya-traingame-ft.zip -d ~/laya-traingame-ft
python3 tools/laya-finetune/finetune.py --check-checkpoint ~/laya-traingame-ft   # layout, senza torch

# Tabella di valutazione (scenari ovvi + partita, azioni e deliberazioni) con la checkpoint nuova:
cargo run -p sim-laya --release --features metal,accelerate --example laya_eval -- \
    --model-dir ~/laya-traingame-ft
# (equivale a LAYA_MODEL_DIR=~/laya-traingame-ft; senza --model-dir né variabile usa la base da HF)

# Nel gioco:
LAYA_MODEL_DIR=~/laya-traingame-ft cargo run -p game --release --features laya-metal
```

- Le temperature fittate si applicano da sole: `LayaModel` legge `temperature` e
  `temperature_by_options` da `rl_agent_config.json` (`AgentConfig::temperature`) e nessuna
  parte del gioco le sovrascrive.
- Le soglie del cervello vanno ritarate su una checkpoint calibrata: `min_confidence` 0.5,
  `deliberation_min_confidence` 0.6, e il peso della regola 0.5 del gioco (`GAME_PRIOR_WEIGHT`)
  erano scelti per lo zero-shot.
- Con un modello che batte le regole conviene provare peso della regola 0 e soglie più basse
  dalla finestra **Cervello** (tasto B), guardando la tabella delle soglie di `laya_eval`.

## 5. Cosa aspettarsi, onestamente

- **Il guadagno non è noto finché non si fa girare.**
  - Upstream, sul benchmark typed-decisions, il fine-tuning porta da 0.36 a 0.77 di accuratezza.
  - Qui partiamo da circa il 40% sui casi ovvi e il 22–45% di accordo.
  - Il tetto è l'insegnante: `UtilityBrain` e la regola. Un Laya messo a punto su queste
    etichette al massimo le **imita**, più morbido e più lento. Non le supera, a meno di
    aggiungere etichette migliori (scritte a mano o da un LLM) nello stesso formato.
- **I casi ovvi di `laya_eval` (a) condividono i modelli con quelli del train.**
  - Mondi diversi (semi 1–3 esclusi), ma stesse situazioni costruite.
  - Un buon risultato su (a) dice che ha imparato quei casi, non che generalizza.
  - Le misure più oneste sono l'accordo sulla partita (b) e il test split di `finetune.py`.
- **Da guardare dopo il training:**
  - l'accuratezza per posizione dell'etichetta, che deve essere piatta;
  - l'ECE dopo la calibrazione, che deve essere bassa;
  - in `laya_eval`, i tipi dove lo zero-shot crollava: mangia e compra per le azioni, furto e
    protesta per le deliberazioni.

## Problemi comuni

- `CUDA out of memory`: `--micro-batch 4 --grad-accum 8 --grad-ckpt`.
- Kaggle senza GPU o senza Internet: verifica il telefono dell'account.
- Troppo lento: `--mode head`, `--epochs 2` o `--max-train-rows 10000`.
- `pip` non trova `laya @ git+…`: sostituisci la riga con `laya==0.3.21`.
- Sessione chiusa a metà: la versione salvata di Kaggle continua a girare; in Colab rilancia
  con meno righe o epoche.

## Test

```bash
python3 tools/laya-finetune/test_finetune_data.py        # solo stdlib, senza torch né GPU
python3 -m py_compile tools/laya-finetune/*.py
cargo test -p sim-laya --test dataset                    # schema del JSONL dal lato Rust
```

## Riferimenti upstream

- Laya ([NandhaKishorM/laya](https://github.com/NandhaKishorM/laya), Apache-2.0), commit
  `9d955671415fc19f069b9cc998928075c1f255ec` (v0.3.21, 2026-09-27):
  - `notebooks/laya_finetune_typed_decisions_2xT4_kaggle.ipynb`: celle 6 (preprocessing) e 8
    (`train_ddp.py`: RLCD, fit delle temperature, salvataggio);
  - `docs/finetune.md`;
  - `laya/common.py`: `build_model`, `build_sequence`, `render_options`, `proper_reward`,
    `ece_score`, `temp_bucket`, `clamp_temperature`;
  - `laya/agent.py`: `_fix_tokenizer_config`, `_to_internal`;
  - `laya/revisions.py`.
- Upstream non ha un comando di training installabile: la ricetta vive solo nel notebook.
  `finetune.py` ne riprende il ciclo su una GPU e usa le funzioni della libreria `laya`.
- Pesi: [`convaiinnovations/laya-multilingual`](https://huggingface.co/convaiinnovations/laya-multilingual)
  @ `e4e9ddf21a7b1903b7acffd8814ad4307bf63a67` (`PINNED_REVISIONS` di `laya/revisions.py`).
