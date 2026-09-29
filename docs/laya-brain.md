# LayaBrain: fattibilità e integrazione di Laya nel `sim`

Ricerca del 2026-09-28 su Laya **0.3.21** (repo `main`, PyPI `laya` 0.3.21).
Legenda: **[V]** = fatto verificato alla fonte linkata · **[I]** = mia inferenza/stima, da misurare.

---

## TL;DR

- **[V]** Laya è un encoder bidirezionale (ModernBERT-large / mmBERT-base) + una piccola "decision head" di 2 layer transformer. Un forward pass restituisce una distribuzione di probabilità sulle opzioni di una domanda `choice` / `score` / `noul` (sì/no). Nessuna generazione di testo.
- **[V]** Per l'italiano va usato il checkpoint `laya-multilingual` (mmBERT-base, 322M). Il checkpoint inglese applicato all'italiano crolla: 0.34 di accuratezza con una confidenza media di 0.97.
- **[V]** I checkpoint base sono **quasi a caso zero-shot** su decisioni di dominio (0.36 contro 0.32 del caso). Il valore arriva col **fine-tuning**, e lo dicono gli autori stessi: "a fast base to specialise, not a zero-shot decision engine".
- **[I]** Su un Mac Apple Silicon ci si può aspettare **10¹–10² decisioni/s**, non migliaia. Con migliaia di NPC Laya non può decidere per tutti a ogni tick. Va usato come "tie-breaker" asincrono su un sottoinsieme (vicino al giocatore, pareggi di utilità), con `UtilityBrain` come fallback.
- **Raccomandazione:** (a) prima un prototipo con un server Python locale, solo per validare la qualità. Poi (c) inferenza in-process con **candle + Metal** caricando direttamente le safetensors ufficiali. L'opzione (b), ort/ONNX, è un'alternativa valida solo se la PR ufficiale Rust #672 viene mergiata, ma su Mac girerebbe di fatto su CPU.
- **[V]** Il repo ha **10 giorni di vita** e l'API cambia ogni giorno: 21 release e 712 commit tra il 18 e il 28 settembre. Conviene fissare la revisione HF e trattare tutto come sperimentale.

---

## 1. Come si invoca Laya

### Input [V]
Una chiamata prende uno **state** (stringa, oggetto JSON o lista di turni) e un dizionario di **questions** tipizzate ([README](https://github.com/NandhaKishorM/laya/blob/main/README.md#quickstart)):

```python
questions = {
  "azione": {"type": "choice",
             "instructions": "Cosa fa adesso Marta?",
             "criteria": {"A": "mangia alla Mensa (carrozza 2)",
                          "B": "dorme nella cuccetta (carrozza 5)"}},
  "urgenza": {"type": "score", "instructions": "Quanto ha fame?",
              "criteria": ["per niente", "un po'", "moltissimo"]},
  "ruba":    {"type": "noul", "instructions": "Ruberà del cibo?"},
}
router.predict("Marta, 34 anni, cuoca. Ore 13:10, carrozza 3. Fame alta, stanchezza bassa.", questions)
```

- **`choice`**: restituisce l'etichetta top, le `probabilities` per opzione, `confidence` (1 − entropia normalizzata) e `answer_confidence` (max p, la grandezza calibrata).
- **`score`**: restituisce il livello atteso sulla scala ordinale e la sua distribuzione.
- **`noul`**: restituisce P(true).
- Si può anche usare `decide(state, schema=JSON Schema)`: enum → choice, bool → noul, intero limitato → score ([docs/structured.md](https://github.com/NandhaKishorM/laya/blob/main/docs/structured.md)).

### Formato interno di una riga [V]
Da [`laya/common.py::build_sequence`](https://github.com/NandhaKishorM/laya/blob/main/laya/common.py): **una riga del batch corrisponde a una coppia (state, question)**.

```
[CLS] "<tipo> question: <instructions>" [SEP] [MASK] opt0 [MASK] opt1 … [SEP] <state> [SEP]
```

- Ogni opzione viene resa come `"chiave: descrizione"` e troncata a **48 token**.
- La testa legge l'hidden state in corrispondenza di ciascun `[MASK]`. Sequenza di calcolo: `h + type_emb[qtype]` → 2 layer `nn.TransformerEncoderLayer` (pre-norm, FFN 4d) → `scorer` (LN → Linear → GELU → Linear → 1 logit per opzione).
- Seguono softmax e **temperatura** per (tipo, numero di opzioni) presa da `rl_agent_config.json`.
- Input ONNX del grafo: `input_ids, attention_mask, marker_pos, marker_mask, qtype`. Output: `logits [B, K]` e `act_logits` ([scripts/export_onnx.py](https://github.com/NandhaKishorM/laya/blob/main/scripts/export_onnx.py)).

### Limiti [V]
| Checkpoint | Encoder | Parametri | `max_len` | `head_max_len` (budget opzioni) |
|---|---|---|---|---|
| [`laya`](https://huggingface.co/convaiinnovations/laya) | ModernBERT-large | 421M | 512 | 192 |
| [`laya-multilingual`](https://huggingface.co/convaiinnovations/laya-multilingual) | mmBERT-base | 322M | 1024 (fino a 8192) | 256 |
| [`laya-typed-decisions`](https://huggingface.co/convaiinnovations/laya-typed-decisions) | ModernBERT-large | 421M | 1024 | 192 |

- **Numero di opzioni:** non c'è un limite fisso nel modello, ma tutte condividono `head_max_len`. Oltre circa **20 opzioni** con descrizione le opzioni vengono troncate e l'accuratezza crolla: Banking77 scende a 0.425. Il server HTTP rifiuta più di **100** opzioni per domanda. Per molte opzioni c'è `predict_shortlist` (top-k via embedding). Fonte: [README, "Honest limits"](https://github.com/NandhaKishorM/laya/blob/main/README.md#honest-limits).
- **Etichette:** conviene usare chiavi opache (`A`, `B`, …) o semantiche. Etichette `true/false/yes/no` dentro una `choice` distorcono la risposta. Il README segnala anche un problema con le **negazioni** ([#377](https://github.com/NandhaKishorM/laya/issues/377)).
- **`score`:** è la primitiva più debole. Il checkpoint multilingual ha un bias di posizione ([#131](https://github.com/NandhaKishorM/laya/issues/131)).
- **`noul`:** sul checkpoint inglese tende a seguire le etichette invece dello state ([#156](https://github.com/NandhaKishorM/laya/issues/156)).
- **`act_probability`:** è inutilizzabile ([#185](https://github.com/NandhaKishorM/laya/issues/185)).

### Lingue: e l'italiano? [V]
- **[V]** mmBERT copre più di 100 lingue. Nella model card di `laya-multilingual` l'italiano è elencato tra le lingue (`it`). Il router manda l'italiano "plain ASCII" al multilingual ([model card](https://huggingface.co/convaiinnovations/laya-multilingual)).
- **[V]** Sweep su MASSIVE intent, 20 opzioni, 100 esempi per lingua, fatto su laya 0.2.0 ([cpu_51_language_sweep.json](https://github.com/NandhaKishorM/laya/blob/main/research/results/cpu_51_language_sweep.json)):

  | Checkpoint | Accuratezza it | Confidenza media it | Accuratezza en |
  |---|---|---|---|
  | multilingual | **0.50** | 0.78 | 0.68 |
  | inglese | 0.34 | 0.97 | 0.82 |

- **[I]** Un'opzione da valutare è generare internamente il contesto NPC **in inglese**, anche con la UI in italiano, e usare il checkpoint inglese: più accurato, ma circa 2–3× più lento. Va misurata sui nostri prompt.

### Calibrazione [V]
- Tutti i checkpoint sono **sovra-confidenti** così come vengono distribuiti.
- Rifittando una temperatura per (tipo, numero di opzioni) l'ECE scende da 0.466 a 0.081 (inglese) e da 0.314 a 0.106 (multilingual).
- **`laya-multilingual` è distribuito con temperature `[1,1,1]`, cioè non fittate.** Per il gating sulla confidenza bisogna fittarle noi. Fonte: [README, "Calibration"](https://github.com/NandhaKishorM/laya/blob/main/README.md#calibration).

## 2. Pesi, formati, licenza [V]

- **Licenza:** Apache-2.0 sia per il codice sia per i pesi.
- **Hugging Face:** `convaiinnovations/laya` (con le sottocartelle `multilingual/` e `typed-decisions/`), `convaiinnovations/laya-multilingual`, `convaiinnovations/laya-typed-decisions`.
- **Formati ufficiali:** **solo `model.safetensors`**, in F16. Dimensioni: 843 MB l'inglese, 644 MB il multilingual. Ogni checkpoint include `encoder/config.json`, `rl_agent_config.json` e `tokenizer/tokenizer.json`; quello del multilingual pesa 34 MB con un vocabolario da 256k (verificato via HF API).
- **Nomi dei tensori:**
  - `encoder.*`: ModernBERT nativo (`Wqkv`, `Wo`, `Wi`, …).
  - `head.layers.{0,1}.*`: formato torch `in_proj_weight`, `linear1/2`, `norm1/2`.
  - Poi `type_emb`, `scorer.{0,1,3}`, `act_head.*`, `temperature`.
- **ONNX:** non è pubblicato su HF, va esportato da sé. Ci sono due strade:
  - [`scripts/export_onnx.py`](https://github.com/NandhaKishorM/laya/blob/main/scripts/export_onnx.py): grafo unico, opset 18, con `--quantize` per una variante INT8 per-canale solo CPU.
  - [`laya-ts/scripts/export_onnx.py`](https://github.com/NandhaKishorM/laya/blob/main/laya-ts/scripts/export_onnx.py): split `encoder.onnx` + `head.onnx`, verificato entro 1e-4.
- **Formati community (non ufficiali):**
  - GGUF F16/Q8_0/Q4 di [monatis/ggmlc](https://github.com/monatis/ggmlc/blob/main/examples/laya/README.md), su HF `mys/laya-multilingual-GGUF`. Include un binario macOS-arm64 con Metal e server HTTP; **il repo non dichiara una licenza**.
  - MLX + CoreML/ANE: [tc3oliver/laya-apple](https://github.com/tc3oliver/laya-apple), in Python.

## 3. Prestazioni riportate [V]

| Setup | Numero | Fonte |
|---|---|---|
| T4, multilingual, 1 domanda | 32.8 ms | [README Speed](https://github.com/NandhaKishorM/laya/blob/main/README.md#speed-tesla-t4-measured) |
| T4, multilingual, 50 domande in un batch | 337 ms (6.8 ms/domanda) | idem |
| T4, throughput batched | 103–332 domande/s | idem |
| CPU, 4 core EPYC (m7a.xlarge), multilingual | ~190–220 ms/domanda, il batching non aiuta | [latency_cpu_m7a_xlarge](https://github.com/NandhaKishorM/laya/blob/main/research/results/latency_cpu_m7a_xlarge_20260924.json) |
| CPU, 4 core, inglese (large) | ~580–720 ms/domanda | idem |
| Apple M-series MPS, `decide_batch` su 8 ticket | 2624 ms in sequenza vs 723 ms in batch (3.6×) | README "Schema-driven" |
| Apple MPS via MCP, 16 ticket misti | 467–477 ms in batch (~29 ms/richiesta) | README "MCP Server" |
| Apple M1 in ONNX fp32 su CPU, 80 ticket × 3 domande | 36.3 s con `sort_by_length` (~150 ms/riga) | README "Batch Mode" |
| M4 Max con MLX (laya-apple), forward singolo, large | 12.2 ms a 128 token, 19.2 ms a 256, 71 ms a 1024 | [laya-apple](https://github.com/tc3oliver/laya-apple) |
| M4 Max con ANE, forward singolo ≤128 token | 9.9 ms | idem |

- **[V]** Nota del README: "On CPU, increasing batch size alone may not speed up inference". Il batching è un vantaggio solo su GPU.
- **[I]** Stima dei costi:
  - mmBERT-base ha circa 125M parametri non-embedding (encoder più testa), quindi circa 0.25 GFLOP per token.
  - Una decisione NPC tipica è di circa 150–250 token: contesto di circa 80 token, più istruzione e 4–6 opzioni brevi.
  - Fanno circa 40–60 GFLOP per decisione.
  - Su una GPU Apple (circa 3.5 TFLOPS fp16 per M1/M2 base, circa 15 per un Max) con efficienza realistica del 30–50%, si ottengono **circa 20–60 decisioni/s su chip base e circa 100–300/s su Pro/Max**.
  - Su CPU si scende a **5–15/s**. Il checkpoint inglese (large) è circa 3× più costoso.

## 4. Opzioni di integrazione da Rust (in ordine di preferenza per questo progetto)

Versioni su crates.io al 2026-09-28 **[V]**:
- `ort` 2.0.0-rc.13 (nessuna release stabile)
- `tokenizers` 0.23.2 stabile, 1.0.0-rc.2 in pre-release
- `candle-core`, `candle-nn`, `candle-transformers` 0.11.0
- `safetensors` 0.8.0
- `hf-hub` 1.0.0

### (c) candle in-process, Metal: **scelta consigliata per il gioco**
**Fatti [V]:**
- `candle-transformers` 0.11 include [`models::modernbert`](https://github.com/huggingface/candle/blob/main/candle-transformers/src/models/modernbert.rs), con RoPE globale e locale, sliding window e il layer 0 senza `attn_norm`.
- Esistono già tre port Rust su candle con licenza Apache-2.0, nati tra il 19 e il 20 settembre, con 4–9 stelle ciascuno:
  - [b0xtch/laya-candle](https://github.com/b0xtch/laya-candle): kernel Metal ottimizzati, carica le safetensors originali, batch da 16 di default.
  - [apiplant/laya-rs](https://github.com/apiplant/laya-rs): batch con domande eterogenee.
  - [aovestdipaperino/laya-rust](https://github.com/aovestdipaperino/laya-rust): CPU, Metal e CUDA.

**Lavoro necessario [I]:**
1. Costruire a mano `modernbert::Config`. I `config.json` sono in formato transformers-5, con `rope_parameters` annidato, e non combaciano con i campi serde di candle (`global_rope_theta`, `local_rope_theta`). Ad esempio l'inglese usa 160000 per la RoPE globale e 10000 per la locale, il multilingual 160000 per entrambe.
2. Mappare il prefisso dei pesi `encoder.*` su `model.*` con `VarBuilder::rename_f`.
3. Scrivere a mano la testa: 2 layer pre-norm con MHA `in_proj` + ReLU (il default di torch), `gather` sui marker, scorer. Sono circa 150 righe.
4. Portare `build_sequence`, le temperature e il decode.
5. Validare la parità con un set di output "golden" registrati dal Python.

- **Pro:**
  - Niente Python, niente export.
  - GPU Metal nel processo del gioco.
  - Batch eterogeneo libero: ogni NPC ha le sue opzioni, basta fare padding su `marker_pos` e `marker_mask`.
  - Pesi F16 caricati direttamente.
- **Sforzo [I]:** 3–6 giorni partendo da uno dei port come riferimento (o come dipendenza, se l'API conviene).
- **Rischi [I]:**
  - Le prestazioni di candle su Metal sono meno curate di MLX/PyTorch.
  - La GPU è condivisa con il rendering di Bevy (wgpu/Metal): possibili stutter, per cui va limitato il batch per frame o ci vuole un worker a bassa priorità.
  - Il port va tenuto allineato a un upstream che cambia ogni giorno. Mitigazione: fissare la revisione dei pesi HF e i golden.

### (b) ONNX + `ort` + `tokenizers`: alternativa solida se la PR ufficiale entra
**Fatti [V]:**
- La [PR #672](https://github.com/NandhaKishorM/laya/pull/672) (issue [#674](https://github.com/NandhaKishorM/laya/issues/674)), aperta il 2026-09-28 e non ancora mergiata, propone la crate `laya-onnx`.
  - Dipendenze: `ort 2.0.0-rc.13` e `tokenizers 0.23`.
  - Parità con Python 0.3.21 entro 2e-4, con golden inclusi. Supporta i tre checkpoint, il router e la shortlist.
  - Richiede di esportare l'ONNX da sé.
- `ort` ha un execution provider CoreML (feature `coreml`, `ep::CoreML` con `ComputeUnits::CPUAndNeuralEngine`) ([docs ort](https://ort.pyke.io/perf/execution-providers)).

**Rischi [I]:**
- Con shape dinamiche e l'attenzione sliding-window di ModernBERT, l'EP CoreML probabilmente partiziona il grafo e ricade in buona parte sulla CPU. Su Mac va quindi considerato **di fatto CPU**, circa 5–15 decisioni/s; l'INT8 può aiutare circa 1.5–2×.
- `ort` è ancora in release candidate e porta con sé la dylib di onnxruntime, circa 30 MB o più.

- **Sforzo:** circa 1–2 giorni se si usa `laya-onnx`, circa 4–6 se lo si porta da sé.

### (a) Server Python locale + HTTP da un thread Rust: **solo per prototipare**
**Fatti [V]:**
- `pip install "laya[serve]"` e poi `laya-serve` espone `POST /v1/systemone`.
  - Una richiesta contiene **un solo state**.
  - Non c'è un endpoint batch.
  - Esegue un solo forward alla volta e restituisce 503 oltre `LAYA_MAX_CONCURRENT` ([docs/http-api.md](https://github.com/NandhaKishorM/laya/blob/main/docs/http-api.md)).
- `examples/server.py` ha invece `/predict/batch`.
- `Router.predict_batch` raggruppa per **schema di domande identico**: con opzioni diverse per NPC il batching si frammenta.
- Su Mac si può usare `device="mps"`.

**Consiglio [I]:** uno script FastAPI da circa 50 righe (o JSONL su stdin/stdout) che chiama `agent.predict_batch` raggruppando per set di opzioni, più `reqwest`/`ureq` da un thread Rust.
- **Sforzo:** mezza giornata.
- **Rischio:** non distribuibile con il gioco (richiede Python ≥3.10, torch e 1–2 GB). Va bene però per misurare la qualità sui nostri prompt prima di investire in (c).

## 5. Come infilarlo nel `decide` batched

### Encoding
- **[I]** Ogni `DecisionRequest` diventa **una riga**:
  - `state` = la stringa di contesto dell'NPC;
  - question = `choice` con `instructions` = "Quale azione sceglie {nome} adesso?";
  - `criteria` = `{"A": options[0].description, "B": …}`.
  - La risposta è `argmax` → indice.
- **[I]** In un'implementazione propria (candle/ort) le righe con numeri di opzioni diversi vanno nello **stesso forward**:
  - padding di `input_ids` alla lunghezza massima del batch;
  - `marker_pos` e `marker_mask` portati a `K_max`;
  - le opzioni mascherate prendono logit −1e4.
  - È esattamente ciò che fa `collate_items` ([common.py](https://github.com/NandhaKishorM/laya/blob/main/laya/common.py)).
  - Ordinare per lunghezza (`sort_by_length`) riduce il padding: 1.5–2.15× misurato ([research/README](https://github.com/NandhaKishorM/laya/blob/main/research/README.md#length-batching)).
- **[I]** Nessun riuso della cache tra NPC: l'encoder è bidirezionale e lo state sta in coda alla sequenza, quindi ogni riga va ricalcolata per intero. L'unico risparmio è **tenere corte le stringhe**:
  - contesto sotto gli 80 token;
  - descrizioni delle opzioni sotto i 12 token;
  - al massimo 6 opzioni.

### Ordine di grandezza [I]
Con 5 000 NPC che decidono in media una volta ogni 60 s reali servono circa 83 decisioni/s. Sono al limite di un Mac Pro/Max e oltre la portata di un chip base o della CPU. Quindi:
1. **Pre-filtro con UtilityBrain:** a Laya arrivano solo le top-k (≤ 5) opzioni plausibili, così servono meno token e si evitano scelte assurde.
2. **Budget:** N righe per secondo e priorità agli NPC vicini o visibili al giocatore, a quelli narrativamente rilevanti e alle decisioni con utilità quasi pari.
3. **Cache** su `hash(contesto discretizzato + opzioni)`: bisogni in fasce e ora in slot orari fanno ripetere molti contesti.
4. **Gating:** se `answer_confidence` è sotto la soglia θ si usa il fallback. La soglia va calibrata dopo aver fittato le temperature.

### Game loop non bloccante [I]
Il trait attuale restituisce `Vec<usize>` in modo sincrono, mentre Laya risponde in decine o centinaia di ms. Ci sono due strade:
- **Senza cambiare il trait:** `LayaBrain` risponde *subito* con la scelta del fallback e mette la richiesta in coda. Quando arriva il risultato Laya, lo salva come "intenzione" e lo applica alla prossima `decide` per quell'NPC, se l'impronta delle opzioni coincide ancora.
- **Consigliata:** una piccola estensione del trait, ad esempio `enum Decision { Chosen(usize), Pending }` oppure `submit()` + `poll()`. In questo modo l'NPC resta nell'azione corrente, "sta pensando", finché la risposta non è pronta o scade un timeout, dopo il quale si usa il fallback.

**Determinismo:** `sim` usa `rand_chacha` serializzabile. Risultati asincroni che dipendono dai tempi della GPU **rompono la riproducibilità** e i replay. Per riprodurre una partita bisogna registrare le decisioni applicate (tick, NPC, indice) o eseguire in modalità sincrona nei test.

## 6. Maturità e rischi [V salvo dove indicato]

- **Repo:**
  - creato il **2026-09-18**, 712 commit, 93 contributor;
  - 21 release fino alla v0.3.21 del 2026-09-27;
  - 27.4k stelle e 2.4k fork;
  - 56 issue aperte e 83 chiuse, 108 PR aperte ([GitHub API](https://api.github.com/repos/NandhaKishorM/laya)).
  - **[I]** È una crescita esplosiva con un'API molto instabile: ci sono stati cambi di default anche nelle ultime release.
- **Qualità zero-shot:**
  - Sulla suite typed-decisions i checkpoint base stanno **sotto la baseline "classe di maggioranza"**: 0.362 e 0.352 contro 0.461. Il 0.766 pubblicizzato è del checkpoint fine-tuned.
  - Per il nostro dominio servirà quasi certamente un **fine-tuning**: il notebook Kaggle su 2×T4 impiega circa 4–5 h per 30k domande. La pipeline è nel §10.
  - **[I]** In alternativa si può addestrare solo una testa leggera su encoder congelato, come fa [stuntd](https://github.com/bladedevoff/stuntd), usando come etichette le decisioni di UtilityBrain o di un LLM "insegnante".
- **Calibrazione:** le affermazioni di calibrazione valgono *dopo* il fit delle temperature sul proprio dominio. Il multilingual non è calibrato. Nella config inglese c'è una temperatura `choice:11+` = 0.10, che quindi rende le probabilità più estreme invece di ammorbidirle; va tenuto presente se si usano molte opzioni.
- **Bias noti:** etichette `noul`, bias di posizione su `score`, negazioni, collasso del checkpoint inglese fuori dall'inglese con alta confidenza (Khmer: 0.000 di accuratezza con 0.952 di confidenza).
- **Benchmark vs Jev:** i numeri di Jev sono di terze parti e non misurati dagli autori, lo dicono loro stessi.

## 7. Percorso raccomandato

1. **Fase 0, spike di 1–2 giorni con l'opzione (a)** su Python e MPS.
   - Preparare 200–500 decisioni NPC reali estratte dal `sim`, etichettate a mano o da un LLM.
   - Confrontare multilingual+italiano, inglese+contesto inglese e UtilityBrain.
   - Misurare decisioni/s sul Mac di sviluppo.
   - **Gate:** se lo zero-shot non batte UtilityBrain in modo percepibile, si passa al fine-tuning o si abbandona.
2. **Fase 1, eventuale:** fine-tuning e fit delle temperature sul dominio TrainGame, poi pubblicare il checkpoint privato su HF.
3. **Fase 2, opzione (c):** crate `sim-laya` separata (per non portare candle dentro `sim`) con candle 0.11 + feature `metal`.
   - Riferimenti: `modernbert` di candle-transformers e il port [laya-candle](https://github.com/b0xtch/laya-candle).
   - Test di parità contro i golden Python.
   - Se la PR #672 viene mergiata e il throughput su CPU basta, (b) `laya-onnx` resta un'alternativa a basso sforzo.

## 8. Bozza di design di `LayaBrain`

```rust
// crate `sim-laya` (dipende da `sim`, candle-*, tokenizers); `sim` resta senza dipendenze ML.
pub struct LayaBrain<F: Brain> {
    fallback: F,                                   // UtilityBrain
    tx: crossbeam_channel::Sender<Job>,            // verso il worker
    rx: crossbeam_channel::Receiver<Answer>,       // dal worker
    ready: HashMap<NpcId, Answer>,                 // risposte arrivate, non ancora consumate
    in_flight: HashSet<NpcId>,
    cache: lru::LruCache<u64, usize>,              // hash(contesto discretizzato + opzioni) -> indice
    budget_per_call: usize,                        // righe max accodate per decide()
    min_conf: f32,                                 // soglia su answer_confidence
    top_k: usize,                                  // opzioni passate a Laya (pre-filtro utility)
}

struct Job { npc: NpcId, fingerprint: u64, context: String, options: Vec<String>, map: Vec<usize> }
struct Answer { npc: NpcId, fingerprint: u64, idx: usize, conf: f32 }

impl<F: Brain> Brain for LayaBrain<F> {
    fn decide(&mut self, world: &World, reqs: &[DecisionRequest]) -> Vec<usize> {
        for a in self.rx.try_iter() { self.in_flight.remove(&a.npc); self.ready.insert(a.npc, a); }
        let mut out = self.fallback.decide(world, reqs);      // risposta immediata, mai bloccante
        let mut budget = self.budget_per_call;
        for (i, r) in reqs.iter().enumerate() {
            let fp = fingerprint(world, r);
            if let Some(&idx) = self.cache.get(&fp) { out[i] = idx; continue; }
            match self.ready.remove(&r.npc) {
                Some(a) if a.fingerprint == fp && a.conf >= self.min_conf => {
                    self.cache.put(fp, a.idx); out[i] = a.idx;
                }
                _ if budget > 0 && !self.in_flight.contains(&r.npc) && worth_asking(world, r) => {
                    let job = build_job(world, r, fp, self.top_k);  // top-k via utility, etichette A..E
                    if self.tx.try_send(job).is_ok() { self.in_flight.insert(r.npc); budget -= 1; }
                }
                _ => {}
            }
        }
        out
    }
}

// Worker (std::thread dedicato, possiede modello + tokenizer):
// loop { raccogli Job fino a B=32 righe o 15 ms; ordina per lunghezza; build_sequence per riga;
//        pad input_ids/attention_mask, marker_pos/marker_mask a K_max; forward (Metal f16);
//        logits/T[qtype,K] -> softmax -> argmax, answer_confidence; rimappa via Job.map; send(Answer) }
```

Note di design [I]:
- `worth_asking` stabilisce le priorità: vicinanza al giocatore, margine piccolo tra le prime due opzioni di utilità, NPC con un "ruolo".
- Un indice restituito tardi è valido solo se il `fingerprint` coincide ancora, cioè se opzioni e contesto non sono cambiati.
- Per i replay deterministici si registrano le coppie (tick, NPC, idx) applicate, oppure si usa `LayaBrain` in modalità sincrona nei test.
- Conviene un `max_len` basso, per esempio 256: le righe NPC sono corte e la latenza dipende dalla lunghezza reale della sequenza.

## 9. Stato dell'implementazione (M7)

### Cervello ibrido (`crates/sim-laya/src/brain.rs`)
- **`LayaBrain<F: ScoredBrain>`** implementa `sim::Brain` sopra un `ChoiceModel` con `UtilityBrain` come ripiego. Segue la bozza del §8:
  - `decide` non blocca mai;
  - un worker su un thread dedicato raccoglie lotti di al massimo `batch_rows` righe o `batch_wait` e chiama `predict_batch`;
  - si usano budget per chiamata, coda massima, pre-filtro top-k (≤ 5, `UtilityBrain::scores`) con rimappatura sugli indici originali, priorità alle carrozze a fuoco (`set_focus`) e alle decisioni con margine piccolo;
  - una cache LRU e l'impronta usano la stessa chiave: contesto discretizzato più le opzioni in forma astratta (`OptionSig`). Le risposte superate (impronta cambiata) o poco sicure (`min_confidence`) si scartano.
- **"Sta pensando":** nel `sim` c'è una piccola aggiunta, la scelta speciale `sim::THINK` più `Brain::think_minutes`: l'NPC ozia per quei minuti e poi decide di nuovo. `LayaBrain` la usa per gli NPC a fuoco con una domanda in volo, al massimo per `max_think_minutes`, poi ricade sul ripiego.
- **Determinismo:**
  - la modalità sincrona (`attach_sync`) chiama il modello dentro `decide` ed è deterministica;
  - in modalità asincrona `record_log` registra ogni decisione (`LogEntry`: tick, NPC, indice, fonte), e `ReplayBrain` la rigioca identica (c'è un test).
- **Salvataggi:** si salva solo il ripiego (`fallback()`, lo stesso `UtilityBrain` di prima: il formato dei salvataggi non cambia). Configurazione, cache e modello sono stato di esecuzione. `replace_fallback` tiene modello e modalità dopo un caricamento.
- **Statistiche (`LayaStats`):**
  - decisioni per fonte (utility/laya/cache/pensa);
  - domande inviate, risposte, fallite, applicate, scartate (poco sicure o superate);
  - usi della cache, accordo col ripiego, latenza media e massima;
  - coda, righe per lotto, istogramma della confidenza.
- **`MockModel`** (`mock.rs`): euristica a parole chiave sul contesto italiano, con latenza configurabile. Serve per i test e per provare il gioco senza pesi.
- **Modello vero:** `loader.rs` è l'unico punto di contatto con `laya::LayaModel` (feature `laya`, `metal`). Senza la feature restituisce un errore che spiega come abilitarla. `LAYA_MODEL_DIR` carica i pesi da una cartella invece di scaricarli.

### Valutazione (`examples/laya_eval.rs`, `src/eval.rs`)
```
cargo run -p sim-laya --example laya_eval                              # utility, random, mock
cargo run -p sim-laya --release --features laya --example laya_eval    # + Laya (CPU)
```
Cosa misura:
- (a) accuratezza su 40 scenari "ovvi" costruiti modificando lo stato di un NPC (5 tipi × 8);
- (b) accordo con `UtilityBrain` su 300 decisioni vere di una partita;
- confidenza media e istogramma, decisioni al secondo.

I modelli vedono solo le top-5 opzioni per utilità. `--model-dir DIR` (o `LAYA_MODEL_DIR`) valuta una checkpoint locale, per esempio una messa a punto (§10). Il contesto è solo italiano: `npc_context` non ha una variante inglese, quindi il confronto con il checkpoint inglese resta da fare.

Risultati del 2026-09-29: seme 1, top-5, contesto italiano. Laya è `laya-multilingual` zero-shot, in release su CPU (feature `laya`, F32, senza Accelerate/Metal).

| Cervello | Ovvi (a) | Accordo (b) | Confidenza media | Decisioni/s |
|---|---|---|---|---|
| UtilityBrain | 100% | 100% (etichetta) | — | ~6·10⁶ |
| UtilityBrain, altro seme | 100% | 93% | — | — |
| RandomBrain | 32% | 14% | — | — |
| MockModel | 100% | 95% | 0.65 | ~4·10⁵ |
| **Laya multilingual** | **42%** | **45%** | **0.43** | **~2** |

Per tipo di scenario, Laya ottiene:
- dorme 88% e lavora 100%;
- mangia 0% e compra 0%;
- chiacchiera 25%.

Il throughput su CPU resta circa 2 righe/s indipendentemente dalla dimensione del lotto (1, 4, 16). Il modello caricato impiega 1–2 s in release e circa 6 s nel gioco in debug. Conferma il §6: zero-shot Laya non batte `UtilityBrain`. Con la soglia di default (0.5) la maggior parte delle risposte viene scartata come poco sicura, quindi nel gioco l'ibrido resta prudente. Prima di usarlo davvero servono il fine-tuning, il fit delle temperature, e la misura con Metal/Accelerate (`--features metal`).

### Nel gioco
- Finestra **Cervello** (tasto **B**, o il pulsante nel pannello del tempo):
  - modalità *Utility* / *Laya (ibrido)* / *Mock (prova)*;
  - stato del modello ("caricamento modello…", "pronto", "errore: …", oppure "non disponibile" senza la feature);
  - statistiche, cursori per budget, coda, soglia, top-k e margine, e l'opzione "chi è in vista aspetta Laya".
- Il fuoco sono le carrozze visibili dalla camera.
- L'ispettore mostra chi ha deciso l'azione corrente (UtilityBrain, Laya, cache, in attesa) e, per Laya, la confidenza e le opzioni con le probabilità.
- Comandi:
  - `cargo run -p game` (Laya indisponibile);
  - `cargo run -p game --features laya` (CPU) oppure `--features laya-metal`;
  - `TRAINGAME_BRAIN=laya|mock` sceglie la modalità all'avvio.

### Deliberazioni: Laya per le scelte rare (M8)

Le deliberazioni del `sim` (proposta di coppia, avere un figlio, furto, protesta) sono rare (~5 al giorno di gioco con 400 NPC), descritte a parole, con 2–4 opzioni e una scadenza di 2–6 ore di gioco. Se nessuno risponde, alla scadenza decide la regola del `sim` (una scelta pesata e casuale, `World::deliberation_rule_weights`). Sono il caso adatto a un modello lento: poche domande, e c'è tempo.

**`LayaBrain`** (`brain.rs`, pezzi puri in `deliberation.rs`):
- `answers_deliberations()` è vero con Laya acceso (`enabled`), `LayaConfig::deliberations` (default vero) e un modello pronto o in caricamento (sempre in modalità sincrona). Altrimenti, come con `UtilityBrain`, le regole decidono subito, e la partita è identica a una senza Laya (c'è un test).
- La domanda è `ChoiceQuery { context, instructions: question, options: descrizioni }`, senza lettere (`deliberation_query`).
- Il worker ha **due code**. Le deliberazioni si servono per prime, e prima quelle delle carrozze a fuoco. I lotti di azioni si eseguono a pezzi di 8 righe: se nel frattempo arriva una deliberazione, il resto torna in coda. Con Laya vero una deliberazione aspetta quindi al più circa mezzo secondo di azioni. In modalità sincrona la risposta arriva nello stesso tick in cui la deliberazione si apre.
- Miscela facoltativa con la regola: `p = (1 − w)·Laya + w·regola`, con `w = prior_weight` (default 0, solo Laya; nel gioco 0.5, vedi sotto). La regola usata è quella al momento dell'apertura.
- Soglia a parte, `deliberation_min_confidence` (default 0.6). Sotto la soglia non si risponde e la regola decide alla scadenza. Una risposta arrivata dopo la scadenza (o per una deliberazione annullata) non raggiunge mai il mondo: `answers_ignored` resta 0, e le domande ancora in coda per deliberazioni chiuse si tolgono dal worker.
- Le deliberazioni aperte che il cervello non ha mai visto (modello caricato dopo, `reset()`, partita caricata) si chiedono al primo tick utile.
- `DeliberationStats` (in `LayaStats::deliberations`), per tipo, conta: domande, risposte, date, sotto soglia, in ritardo, chiuse. Ci sono anche gli errori, l'accordo con la scelta più probabile della regola, la latenza media e massima e l'istogramma della confidenza.
- `deliberation_info(id)` restituisce stato, probabilità di Laya, miscela e regola; `pending_deliberations()` elenca le domande in volo.
- Replay: con `record_log`, `take_deliberation_log()` restituisce un `DeliberationLog`, che contiene:
  - i cambi di modalità: quando il cervello rispondeva e quando no, perché cambia il momento in cui decide la regola;
  - le risposte date, con tick e "giro" di `run_deliberations`.
  
  `ReplayBrain::new(&log, think).with_deliberations(delib_log)` rigioca la partita identica (test `async_runs_with_deliberations_replay_exactly`).
- `MockModel` ha un'euristica a parole chiave per le deliberazioni: carattere, affinità, differenza d'età, figli, razioni a persona, gettoni e prezzo, chi può aiutare. È deterministica, così test e gioco funzionano senza pesi.
- Nel `sim` c'è un'unica aggiunta: `World::open_deliberation` è pubblica, per test, valutazione e scenari scritti a mano.

**Valutazione** (`src/eval/deliberations.rs`, seconda parte di `laya_eval`):
```
cargo run -p sim-laya --example laya_eval -- --only-deliberations                                  # regole, caso, mock
cargo run -p sim-laya --release --features metal,accelerate --example laya_eval -- --only-deliberations   # + Laya
```
Opzioni: `--delib-per-kind N` (6), `--delib-sampled N` (120), `--no-deliberations`.

Gli insiemi di scenari:
- **(a)** 52 casi "ovvi", 9 tipi, costruiti modificando un mondo generato e aprendo la deliberazione a mano. Esempi:
  - affinità 0.97 tra coetanei single → accetta;
  - affinità 0.1 e più di 25 anni di differenza → rifiuta;
  - giovani senza figli e con cibo abbondante → prova;
  - 4 figli, 0.3 razioni a testa e pochi gettoni → aspetta;
  - onesto a cui manca 1 gettone → risparmia;
  - 0 gettoni, audace e disonesto → ruba;
  - partner che lo adora e ha 120 gettoni → chiede aiuto;
  - audace in coppia senza figli, nascita negata → protesta;
  - prudente con figli → sopporta o lavora di più.
- **(b)** 120 deliberazioni vere di due partite di 400 NPC (deliberazioni ×3 e, nella seconda, dormitori "pieni" per avere proteste): 23 proposte, 30 figli, 37 furti, 30 proteste. Sono etichettate con la scelta più probabile della regola, quindi misurano l'accordo con la regola, non la correttezza.

Ogni modello si chiama una volta sola; le righe con `prior_weight` 0.5 e le soglie si ricavano dalle stesse probabilità.

Risultati del 2026-09-29, seme 1, Laya `laya-multilingual` zero-shot su Metal (M1), release `metal,accelerate`:

| Cervello | Ovvi (a) | Accordo (b) | coppia | figlio | furto | protesta | conf. media | righe/s |
|---|---|---|---|---|---|---|---|---|
| Regola (argmax) | 100% | 100% | 100% | 100% | 100% | 100% | — | — |
| Regola (estratta, come nel gioco) | 77% | 55% | 48% | 80% | 49% | 43% | — | — |
| Caso | 44% | 30% | 30% | 37% | 19% | 37% | — | — |
| MockModel | 100% | 85% | 70% | 97% | 76% | 97% | 0.76 | ~10⁵ |
| MockModel + regola 0.5 | 100% | 92% | 83% | 100% | 86% | 97% | 0.69 | ~10⁵ |
| **Laya** | **40%** | **22%** | 65% | 33% | 3% | 0% | 0.66 | **5.7** |
| **Laya + regola 0.5** | **83%** | **51%** | 70% | 90% | 8% | 50% | 0.53 | 5.7 |

Soglie, con celle "risposte sopra soglia / accuratezza (a) / accordo (b)":

| | ≥ 0.5 | ≥ 0.6 | ≥ 0.7 | ≥ 0.8 |
|---|---|---|---|---|
| Laya | 92% / 41% / 22% | 62% / 42% / 19% | 39% / 38% / 16% | 16% / 40% / 8% |
| Laya + regola 0.5 | 46% / 100% / 89% | 27% / 100% / 100% | 10% / 100% / 100% | 3% / 100% / — |

Cosa si vede:
- **Zero-shot Laya sceglie quasi a caso** anche sulle deliberazioni (40% sui casi ovvi, contro il 44% del caso). Ha preferenze fisse per tipo, che quasi non dipendono dal contesto: in un furto sceglie sempre "ruba" (100% su "disperato → ruba", 0% su "onesto → risparmia"), e non protesta mai. Tra i casi ovvi va bene solo "affini e coetanei → accetta" (100%).
- La **confidenza non aiuta**: sopra 0.8 l'accuratezza resta al 40%.
- Due varianti del prompt, provate e scartate:
  - la situazione prima del contesto: (a) 38%, (b) 24%;
  - solo carattere e situazione, senza lo stato dell'NPC: (a) 33%, (b) 38%, ma con il doppio delle righe al secondo (12/s).
- **Con la regola a peso 0.5 e soglia 0.6** passa il 27% delle risposte, tutte giuste su (a) e d'accordo con la regola su (b). Laya decide così solo quando è d'accordo con la regola, e al posto dell'estrazione casuale della regola sceglie la sua opzione più probabile. Tutto il resto va alla regola alla scadenza. È la configurazione del gioco (`GAME_PRIOR_WEIGHT = 0.5`, soglia 0.6). La libreria resta a `prior_weight = 0`, "solo Laya", da usare con un modello messo a punto.
- **Velocità:** le righe delle deliberazioni sono lunghe (contesto più situazione), quindi Laya fa ~6 righe al secondo su Metal, contro ~15 per le azioni. Una deliberazione sola impiega ~0.2 s: con ~5 deliberazioni al giorno di gioco il carico è trascurabile.
- **Prossimo passo:** serve un fine-tuning su deliberazioni etichettate, per esempio con le estrazioni della regola o con scelte scritte a mano. Solo così Laya può fare meglio della regola invece di limitarsi a confermarla.

**Nel gioco:**
- **Tregua** (`sim_bridge::DeliberationGrace`, attiva di default). Mentre un NPC delle carrozze a fuoco aspetta Laya per una deliberazione, la velocità scende al più a 60 minuti di gioco al secondo. Vale per al massimo 3 s reali per ogni deliberazione. Il pannello del tempo mostra "Marta ci pensa… · rallentato: 1 decisione in corso (60 min/s)". Interruttore, tetto e durata sono nella finestra Cervello.
- **Fumetti** (`bubbles.rs`), in pixel art generata con `Canvas` e testo nel font del gioco:
  - "…" con un'icona (cuore, bebè, moneta, pugno) sopra chi ha una deliberazione aperta;
  - per 3 s il fumetto con la risposta ("Sì!", "Non ancora...", "Ruba!", "Protesto!"), in blu se ha deciso Laya;
  - un cartello sopra la protesta in corso ("Protesta: nascite! (N)").
  
  Con le sole regole le deliberazioni si chiudono nello stesso minuto: si vedono solo i fumetti con la risposta.
- **Ispettore → "In mente"**: domanda, opzioni con le probabilità di Laya e della regola affiancate, chi deciderà, scadenza, ultime decisioni dell'NPC.
- **Finestra Cervello → "Deliberazioni"**:
  - statistiche per tipo e accordo con le regole;
  - interruttore, soglia e peso della regola;
  - la tregua;
  - le ultime 20 decisioni: nome cliccabile, tipo, scelta, chi ha deciso e confidenza.
- **Notifiche** per proposte accettate (una sola, non anche "si sono messi insieme"), ladri sorpresi, proteste convocate e concessioni dell'amministrazione. Seguono i filtri del registro.
- **Storia**: nelle biografie le deliberazioni si leggono dal punto di vista della persona ("Proposta di coppia: accetta… (con Laya, 72%)", "Marta risponde alla sua proposta: …"). Le domande ("ci pensa") si nascondono con "solo i fatti della vita".

## 10. Fine-tuning (M9)

Lo zero-shot non basta (§9): ~40% sui casi ovvi, sia per le azioni sia per le deliberazioni. La pipeline qui sotto rende il fine-tuning ripetibile su una GPU gratuita e ne rimette il risultato nel gioco. Istruzioni passo passo, per Kaggle e Colab, in [`tools/laya-finetune/README.md`](../tools/laya-finetune/README.md).

```
export_dataset (Mac, ~20 s)  →  train/val/test.jsonl  →  finetune.py (Kaggle/Colab T4)  →  laya-traingame-ft/  →  LAYA_MODEL_DIR / laya_eval --model-dir
```

### Riferimenti upstream [V]
- Laya, commit `9d955671415fc19f069b9cc998928075c1f255ec` (v0.3.21, lo stesso del port Rust). Il fine-tuning ufficiale è **solo un notebook**: [`notebooks/laya_finetune_typed_decisions_2xT4_kaggle.ipynb`](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/notebooks/laya_finetune_typed_decisions_2xT4_kaggle.ipynb), descritto in [`docs/finetune.md`](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/docs/finetune.md). Non c'è un comando `laya train` né un modulo di training nel pacchetto.
- **Formato dei dati:** i casi di `LocalLLaMA/typed-decisions` hanno `state`, `questions` (`{id: {type, instructions, criteria}}`) e `gold` (`{id: {label, probabilities}}`), e la distribuzione dell'insegnante è il bersaglio. Il preprocessing chiama `laya.common.build_sequence` e produce item `{ids, markers, qtype, target, label}`.
- **Ricetta RLCD** (`train_ddp.py` nel notebook):
  - soft cross-entropy sulla distribuzione;
  - policy gradient GRPO-style: 4 campioni di logit rumorosi, rumore 0.4 → 0.1, ricompensa `proper_reward` con sferica 0.75 e RPS 1.0;
  - AdamW, learning rate 2.5e-5 per l'encoder e 1e-4 per la testa, cosine, fp16, clip 1.0, 4 epoche.
- **Calibrazione:** una temperatura per tipo, fittata su una fetta tenuta **fuori** dal training (≤ 400 righe o il 10%, seme 20260922). Si scrive in `rl_agent_config.json`, **togliendo** i `temperature_by_options` ereditati.
- **Salvataggio:** `model.safetensors` in F16, `encoder/`, `tokenizer/` e `rl_agent_config.json`: lo stesso layout del repository HF, quindi quello di `ModelSource::Dir`.
- **Tempi upstream:** 2×T4, modello inglese large, circa 30k domande × 4 epoche, **4–5 h**. `laya-multilingual` stesso è stato allenato per 4.97 h su una GPU (`rl_agent_config.json`, sezione `training`). L'esempio "browser agent" di upstream allena il 322M su una GPU da 16 GB in circa 1 h ([`docs/finetune_browser_agent.md`](https://github.com/NandhaKishorM/laya/blob/9d955671415fc19f069b9cc998928075c1f255ec/docs/finetune_browser_agent.md)).
- **Pesi di partenza:** `convaiinnovations/laya-multilingual` @ `e4e9ddf21a7b1903b7acffd8814ad4307bf63a67`, lo SHA "reviewed" di `laya/revisions.py` e lo stesso di `download.rs`.

### Dataset (`src/dataset.rs`, `examples/export_dataset.rs`)
```
cargo run -p sim-laya --release --example export_dataset -- --out laya-dataset \
    --seeds 10..49 --years 3 --max-rows 20000 --kinds actions,deliberations --labels greedy
```
- **Contenuto**, una riga per domanda `choice`, nel formato dei casi `typed-decisions`. Lo schema completo, con una riga d'esempio, è nel README del kit. I criteri sono una **lista** di descrizioni: è la forma senza lettere, come `OptionLabels::Plain` nel gioco. La riga porta anche `kind`, `source`, `case`, `seed`, `split` e `option_tags`.
  - **Azioni:** un campione uniforme (reservoir) delle decisioni di `UtilityBrain` in partite da 300 NPC, con la stessa domanda di `LayaBrain` (top-5 per utilità). L'insegnante è `softmax(utilità / 0.05)`: con i default la sua confidenza media è 0.80.
  - **Deliberazioni:** l'insegnante è la distribuzione della regola del `sim`. Frequenza ×3, proteste più probabili, dormitori "pieni" nei semi dispari.
  - **Casi ovvi** di `eval.rs` e `eval/deliberations.rs`, con distribuzione uniforme sulle risposte giuste, ripetuti 2 volte nel train.
- **Etichette:** `--labels greedy` usa l'opzione più probabile, `--labels sample` un'estrazione, come fa il `sim` con la regola. `gold.probabilities` resta sempre la distribuzione.
- **Pulizia:**
  - duplicati tolti;
  - un'opzione con la stessa descrizione di una migliore si toglie dalla domanda. Il `sim` offre a volte due volte "va in Dormitorio … per fare due chiacchiere" (una per amico): 2606 opzioni tolte nell'esportazione di default;
  - al più il 35% di una categoria di etichetta per seme, altrimenti "ozia" sarebbe metà delle azioni.
- **Split per seme**, così nessun mondo sta in due split. I semi 1–3, usati da `laya_eval --seed 1`, sono esclusi di default.
- **Ordine delle opzioni mescolato** riga per riga, con l'etichetta rimappata: le top-k arrivano ordinate per utilità, e Laya ha un bias di posizione. Con i default l'etichetta cade in posizione 0–4 nel 24/25/21/16/15% dei casi. Non è uniforme perché le righe con 2–4 opzioni non hanno le posizioni alte.
- **Numeri con i default** (misurati il 2026-09-29): 40 semi × 36 giorni, 15.7 M decisioni e 2590 deliberazioni viste, **26 494 righe** in 17 s (41 MB).

  | Split | Semi | Righe | Azioni (partita / ovvie) | Deliberazioni (partita / ovvie) |
  |---|---|---|---|---|
  | Train | 32 | 21 614 | 16 000 / 1 280 | 2 138 / 2 196 |
  | Val | 4 | 2 453 | 2 000 / 80 | 239 / 134 |
  | Test | 4 | 2 427 | 2 000 / 80 | 213 / 134 |

  I token stimati sono circa 270 per riga, al massimo circa 370.
- `tests/dataset.rs` rilegge ogni riga con `serde_json` e controlla:
  - lo schema: chiavi, criteri unici, etichetta tra i criteri, probabilità che sommano a 1;
  - gli id unici e ogni seme in un solo split;
  - il mescolamento e il determinismo;
  - che le etichette `sample` non siano sempre avide.

### Training (`tools/laya-finetune/finetune.py`)
- Riprende il ciclo RLCD del notebook su **una** GPU, con le funzioni della libreria `laya` fissata allo stesso commit: `build_model`, `build_sequence`, `proper_reward`.
- `--mode full` (default) allena encoder e testa. `--mode head` congela l'encoder, come l'alternativa "testa leggera" del §6.
- Tiene l'epoca migliore in validazione.
- **Temperature:** una per `choice` e una per bucket (`choice:2`, `choice:3-5`) fittate sulla validazione, con la cross-entropy verso l'insegnante minimizzata su log T in [0.5, 5], cioè i limiti di Laya e di `clamp_temperature`. Scritte in `rl_agent_config.json` al posto di quelle ereditate.
- **Valutazione sul test**, per la base zero-shot e per la checkpoint nuova:
  - misure: accuratezza sull'etichetta, accuratezza morbida, ECE, NLL e Brier;
  - gruppi: per tipo, per origine, per caso ovvio e per posizione dell'etichetta.
- **Controlli sulla checkpoint:** il layout (gli stessi file di `CheckpointFiles::in_dir`, gli stessi tensori della base), la parità con `laya.load` e lo zip da scaricare.
- Le parti senza torch sono coperte da `tools/laya-finetune/test_finetune_data.py`: lettura del dataset, fit delle temperature, ECE e controllo del layout, provato anche sulla checkpoint sintetica `testdata/tiny`.
- **[I] Tempi** su una T4 per circa 26k righe × 3 epoche: circa 45–90 min in `full`, circa 20–40 min in `head`. Lo script stampa una stima dopo 20 lotti. Costo zero su Kaggle (circa 30 h GPU a settimana) o Colab gratuito.
- **Non verificato qui:** `finetune.py` non è mai stato eseguito con torch né su GPU. È controllato solo con `py_compile`, i test stdlib e il confronto delle API con il commit upstream.

### Rimetterla nel gioco
```
cargo run -p sim-laya --release --features metal,accelerate --example laya_eval -- --model-dir ~/laya-traingame-ft
LAYA_MODEL_DIR=~/laya-traingame-ft cargo run -p game --release --features laya-metal
```
- `loader::load_real_model_from(dir)` carica una cartella locale. `load_real_model`, cioè il gioco, usa `LAYA_MODEL_DIR` se impostata. `laya_eval --model-dir` ha la precedenza sulla variabile.
- Le temperature fittate si applicano da sole (`AgentConfig::temperature`: prima il bucket, poi il tipo).
- Soglie e peso della regola del gioco (0.5/0.6, `GAME_PRIOR_WEIGHT` 0.5) erano scelti per lo zero-shot: vanno rivisti con la tabella delle soglie di `laya_eval`.

### Cosa aspettarsi [I]
- **Il guadagno è ignoto finché non si fa girare.** Upstream passa da 0.36 a 0.77 su typed-decisions, ma là le etichette vengono da un insegnante LLM. Qui l'insegnante è `UtilityBrain` o la regola: il meglio che Laya può fare è **imitarli**, cioè accordo alto su (b), più lentamente di loro. Per fare *meglio* delle regole servono etichette migliori nello stesso formato (a mano, o da un LLM), che l'esportatore può affiancare.
- **I casi ovvi (a) di `laya_eval` usano gli stessi modelli di situazione dei casi ovvi del train**, anche se i mondi (semi) sono diversi. Un salto su (a) va letto come "ha imparato quei casi". Le misure più oneste sono l'accordo (b) e il test split di `finetune.py`.
- **Da controllare dopo il training:**
  - l'accuratezza piatta per posizione dell'etichetta;
  - l'ECE bassa dopo la calibrazione;
  - i tipi dove lo zero-shot crollava: mangia 0%, compra 0%, furto 3%, protesta 0%.
