# Piano: il treno vivo

Piano del 2026-09-29, versione 2. Sostituisce la versione 1 (movimento, dialoghi ed economia), che è stata assorbita qui.

Obiettivi:

1. **Treno più ricco:** tanti tipi di carrozza, anche a **più piani**, con tanti lavori e attività, gestiti meglio.
2. **Movimento reale:** gli NPC camminano tra stanze, piani e carrozze senza mai teletrasportarsi.
3. **Il giocatore diventa un personaggio del treno:** ha la sua stanza e un inventario, gli NPC si ricordano di lui, può lavorare.
4. **Dialoghi:** fumetti tra NPC e una **chat** con cui il giocatore parla con i personaggi e interagisce con loro.
5. **Crafting** in stile Minecraft / The Escapists: si combinano oggetti, alcuni solo a certi banchi da lavoro.
6. **Economia:** produzione specializzata per carrozza, merce trasportata a piedi, prezzi che crescono con la distanza dal produttore, un listino.

Legenda: **[S]** = crate `sim` · **[G]** = crate `game` · **[T]** = test da aggiungere.

> **Aggiornamento 2026-09-29.** Un'altra sessione (branch `claude/train-sim-foundation`) ha già realizzato alcune parti del piano, e queste diventano la base di partenza:
> - **Movimento continuo senza teletrasporto** tra le carrozze (`npc_render.rs`, `smooth_progress`/`TravelPath`), con `travel_minutes_per_carriage = 5`. Della Fase 1 restano stanze, piani e scale.
> - **Fumetti** (`bubbles.rs`), ma solo per le deliberazioni (proposte di coppia, figli, furto, protesta). Restano da fare i dialoghi tra NPC (Fase 5.1–5.3) e la chat.
> - **Economia chiusa** (≈ 6d): tesoreria, salari con feedback, tasse e multe (`world/economy.rs`), più i turni in Mensa con la fila d'attesa. È in corso: la Fase 0 parte dopo il suo commit.

---

## Stato attuale (punto di partenza)

| Area | Cosa c'è | Cosa manca |
|---|---|---|
| Carrozze | 5 `CarriageKind` (Dormitorio, Mensa, Serra, Officina, Mercato) che si ripetono in uno schema fisso di 10 (`world.rs:48`). Una carrozza ha solo `stations` e `stock`: **niente stanze né piani**. Arte procedurale in `env_art.rs` / `prop_art.rs`. | Varietà, piani, stanze, proprietà. |
| Lavori | 4 `Job` (Contadino, Cuoco, Operaio, Mercante); il personale viene riassegnato a mezzanotte (`staff_workforce`, `life.rs:579`). | Varietà, domanda di lavoro, carriera. |
| Movimento | `Travel { to }` sposta `npc.carriage` solo all'arrivo. Il frontend interpola, ma sopra 120 unità di distanza lo sprite scatta (`npc_render.rs:712`) e le velocità dentro e fuori dalla carrozza non combaciano. | Percorsi tra stanze, piani e scale; nessuno scatto. |
| Giocatore | Controller platform in `player.rs` (A/D, salto con W/Spazio). `PlayerInventory` vive **nel `game`**, non nella sim; la sim ha solo `player_take/buy/give`. | Stanza, identità nella sim, relazioni, lavoro, chat. |
| Oggetti | 5 `ItemKind`, `Stock([f32; 5])`, produzione cablata in `World::produce`. | Catalogo ampio, ricette, crafting. |
| Dialoghi | `Socialize` unilaterale, nessun contenuto; `life_fx.rs::chat_hearts` disegna solo un cuoricino. | Conversazioni a due, testo, chat del giocatore. |
| Economia | Prezzo dato solo dallo stock locale (`price_at`, `world.rs:1532`); trasferimenti istantanei; gli acquisti distruggono i gettoni. | Distanza, trasporto, circolazione del denaro, listino. |

Vincoli validi per tutte le fasi:
- `sim` resta senza Bevy e **deterministica**. Il nuovo stato è serde; i test `same_seed_same_state` e `save_load_roundtrip_continues_identically` restano verdi.
- Ogni cambiamento al formato di `World` richiede di alzare `SAVE_VERSION` (`save_file.rs`), perché i salvataggi vecchi vengono rifiutati. Visto quanto cambia il modello, si accetta di **perdere la compatibilità** dei salvataggi durante le fasi 0–2.
- Le nuove opzioni hanno una `description` (le legge `sim-laya`).
- Le prestazioni con 400 NPC e 20+ carrozze restano entro `TICK_BUDGET` (9 ms): ogni fase va controllata con `examples/headless.rs`.
- Ogni fase si sviluppa su un branch GitButler separato.

---

## Fase 0: definizioni come dati [S]

Tutte le altre fasi aggiungono tipi, e oggi ogni tipo è un `match` sparso nel codice. Prima si centralizza.

> **Fatta (2026-09-29).** `sim/src/defs/` contiene `ITEMS`, `RECIPES`, `CARRIAGES` (con postazioni, magazzino e scorte iniziali), `STATIONS` e `JOBS`; `kind.def()` restituisce la riga. `World::produce` sceglie in base a `Work` (`Make`, `MakeScarcest`, `Trade`), non più al lavoro; `World::generate` costruisce postazioni e scorte dalle regole della carrozza.
> - Il comportamento è identico: con gli stessi seed lo stato del mondo dopo 10–30 giorni coincide byte per byte con quello precedente al refactoring (3 configurazioni, fino a 33 carrozze e 700 NPC).
> - Restano fuori dalle tabelle, di proposito: lo schema fisso delle carrozze (`LAYOUT`) e le quote di personale (`job_quotas`, `staff_workforce`), che la Fase 2 riscrive; i contatori dell'economia per oggetto; i testi degli eventi.
> - `Stock` era già `[f32; ItemKind::COUNT]`. Aggiungere un oggetto cambia la forma dei salvataggi: va alzato `SAVE_VERSION`.

- Nuovo modulo `sim/src/defs/` con tabelle statiche in Rust, deterministiche e senza IO:
  - `ItemDef { nome, valore_base, impilabile, durata, deperibile, tag }`
  - `RecipeDef { output, input: &[(ItemKind, u32)], banco: Option<StationKind>, minuti, abilità_minima }`
  - `CarriageDef { nome, piani, stanze: &[RoomTemplate], lavori, produce }`
  - `JobDef { nome, luogo_di_lavoro, postazione, turno, ricette, salario }`
- `ItemKind`, `CarriageKind` e `Job` restano enum (per velocità e serde), ma ogni proprietà si legge dalla tabella. I `match` sparsi (`base_value`, `outlet`, `workplace_kind`…) diventano letture della tabella.
- `Stock` passa da `[f32; 5]` a `[f32; ItemKind::COUNT]`.
- In futuro le tabelle potranno essere caricate da file RON per il modding, ma non subito.
- **[T]** Ogni ricetta usa oggetti esistenti; ogni lavoro ha almeno un tipo di carrozza in cui svolgersi; non ci sono ricette circolari senza una fonte esterna.

---

## Fase 1: spazio (stanze, piani) e movimento reale

> **Fatta in versione semplificata (2026-09-29).** La posizione è carrozza + **piano** (`Npc::floor`, `Station::floor`), non ancora una stanza con un proprietario: le stanze vere arrivano con la Fase 4 (la cabina del giocatore).
> - I Dormitori hanno due piani (`CarriageDef::floors`), con i letti divisi fra i due (`StationRule::spread`). Ognuno torna ogni notte nel suo letto (`Carriage::free_station_for`), così si dorme su entrambi i piani.
> - Chi parte da un piano alto scende prima la scala (`SimParams::stairs_minutes`, 2 minuti); i passaggi tra carrozze sono al piano terra. Si chiacchiera solo con chi è sullo stesso piano (`World::same_place`).
> - Nel gioco gli NPC vanno alla scala, salgono o scendono e poi raggiungono il posto. Il giocatore usa la scala con W/S. La camera segue il piano; Z mostra entrambi i piani.
> - Il prototipo ha scelto la camera che segue il piano come vista normale (decisione aperta 4).

### 1.1 Stanze e piani nella sim [S]
- Una carrozza contiene delle **stanze**:
  ```rust
  pub struct Room {
      pub id: RoomId,              // globale
      pub carriage: CarriageId,
      pub floor: u8,               // 0 = piano terra, 1 = piano di sopra
      pub span: (u8, u8),          // porzione di carrozza in "slot" (es. 0..4 su 8)
      pub kind: RoomKind,          // Cuccette, Cabina, Cucina, Sala, Serra, Bagno, Scale…
      pub owner: Option<Owner>,    // Npc(id) | Player | Famiglia(..)
      pub stations: Vec<StationId>,
  }
  ```
- La posizione di un NPC diventa `Place { carriage, room }` al posto della sola `CarriageId`. Casa, posto di lavoro e letti puntano a stanze.
- **Carrozze a due piani:** i passaggi tra carrozze sono al piano terra, e in una stanza "Scale" si cambia piano. Alcuni tipi (Cabine, Biblioteca, Serra verticale) hanno due piani.

### 1.2 Percorsi [S]
- Funzione pura e deterministica `route(world, from: Place, to: Place) -> Route`, cioè una lista di tratti (`stanza → porta → scale → passaggio …`), ciascuno con i suoi minuti.
- Non si salva nessun percorso: la sim e il frontend ricalcolano lo stesso `route` dagli stessi dati. `Npc::position(now)` restituisce (carrozza, piano, x frazionaria) interpolando lungo il percorso.
- Mentre viaggia, `npc.place` si aggiorna a ogni stanza attraversata, così presenza, occupazione e chiacchierate riflettono la posizione vera.
- **[T]** Un percorso tra due piani passa per le scale; i minuti totali sono uguali alla somma dei tratti; un viaggio da 0 a 4 attraversa 1, 2 e 3.

### 1.3 Rendering e camera [G]
- Una carrozza a due piani ha due interni sovrapposti. Oggi un interno è alto 104 unità e la camera mostra 240 unità in verticale: servono piani da circa 96 unità con un solaio di 8, e la camera che segue **il piano del giocatore** (o dell'NPC seguito) con uno scorrimento verticale morbido. La leggibilità va provata subito con un prototipo, prima di disegnare nuova arte.
- Le scale sono una scala a pioli o una rampa. Il giocatore ci sale con W, e W smette di essere il salto quando si è davanti alle scale.
- **Un'unica velocità di camminata in tempo di gioco** per tutti i tratti: `CARRIAGE_PITCH / travel_minutes_per_carriage` unità per minuto di gioco, cioè 168 a 1× con il valore attuale di 2. Lo scatto di `TELEPORT_DISTANCE` resta solo alla comparsa dello sprite e al caricamento. A 600× gli NPC corrono ma non si teletrasportano.
- Chi va a destra e chi va a sinistra camminano su due corsie leggermente sfalsate.

**Criterio di fatto:** a ogni velocità si vede ogni NPC camminare fino alla destinazione, scale comprese.

---

## Fase 2: carrozze, lavori e gestione

### 2.1 Nuovi tipi di carrozza
Proposta, coerente con l'ambientazione (treno-mondo, rottame, ribelli della coda):

| Carrozza | Piani | Stanze / postazioni | Lavori |
|---|---|---|---|
| Dormitorio | 1–2 | cuccette comuni, bagno | — |
| **Cabine** | 2 | stanze private (quella del giocatore è qui), scale | — |
| Mensa | 1 | tavoli, cucina | Cuoco, Sguattero |
| Serra | 1–2 | aiuole, idroponica | Contadino |
| **Acquario** | 1 | vasche | Pescatore |
| Officina | 1 | banchi, pressa | Operaio |
| **Fonderia** | 1 | forno, riciclo del rottame | Fonditore |
| **Sartoria** | 1 | telai, macchine da cucire | Sarto |
| **Infermeria** | 1 | lettini, laboratorio | Medico, Infermiere |
| **Scuola** | 1 | banchi | Insegnante (i bambini ci passano la giornata) |
| **Biblioteca** | 2 | scaffali, letture (si imparano ricette) | Bibliotecario |
| **Bar** | 1 | bancone, tavoli: il punto d'incontro serale | Barista |
| Mercato | 1 | banchi | Mercante, Facchino |
| **Magazzino** | 1 | scaffalature | Facchino, Magazziniere |
| **Sala macchine** | 1 | caldaia, quadri | Macchinista (produce l'energia per il treno) |
| **Posto di guardia** | 1 | ufficio, cella | Guardia (sorveglia i furti: stile The Escapists) |

- Il treno non segue più lo schema fisso da 10. `World::generate` compone il treno con **regole**: quantità minime per tipo in proporzione alla popolazione; vicinanze sensate (Mensa vicino a Serra o Acquario, Cabine lontane dalla Sala macchine); una sola Sala macchine, in testa.
- Arte: ogni tipo nuovo richiede interni e oggetti procedurali in `env_art.rs` / `prop_art.rs`. È il costo maggiore di questa fase, conviene aggiungere i tipi a gruppi di 3–4.

### 2.2 Gestione migliore
- **Personale su domanda:** a mezzanotte `staff_workforce` assegna i lavoratori in base a cosa manca (scorte basse, malati da curare, bambini da istruire), non con proporzioni fisse. Ogni NPC ha delle **abilità** che crescono lavorando (ricette più difficili, più resa) e delle **preferenze** (non tutti vogliono fare i turni in Fonderia).
- **Stato delle carrozze:** usura (le postazioni si rompono e l'Operaio le ripara), energia (senza la Sala macchine le luci si spengono e la produzione rallenta), pulizia opzionale.
- **Capo carrozza:** l'NPC con più esperienza è il referente. È lui che ti dà lavori e incarichi (Fase 4).
- **[G]** La finestra Carrozze e l'Ispettore mostrano per ogni carrozza: personale richiesto e presente, stato, energia, produzione del giorno.
- **[T]** Nessuna carrozza essenziale (Mensa, Serra, Sala macchine) resta senza personale per più di un giorno se ci sono disoccupati; la popolazione non muore di fame in 30 giorni con i parametri di default.

---

## Fase 3: oggetti e crafting

### 3.1 Catalogo
- Circa 30–40 oggetti divisi in materie prime (Rottame, Cotone, Erbe, Pesce…), semilavorati (Metallo, Tessuto, Filo…), consumabili (Razione, Tè, Medicina, Sapone…), attrezzi con durata (Attrezzo, Ago, Chiave inglese…) e oggetti "da Escapists" (Lima, Corda, Grimaldello, Travestimento…) che servono a missioni ed espedienti.
- Ogni oggetto ha il suo sprite procedurale. Si può partire da un'icona generica colorata per categoria e rifinire in seguito.

### 3.2 Ricette condivise tra NPC e giocatore
- **Gli NPC producono con le stesse ricette** usate dal giocatore: `World::produce` diventa "esegui la ricetta del tuo lavoro alla tua postazione". Una sola logica, e i conti dell'economia tornano.
- Una ricetta può richiedere: gli ingredienti (consumati), un **banco** (Stufa, Banco da lavoro, Telaio, Forno…) oppure niente (si fa a mano, dall'inventario), un'**abilità** minima, e la **conoscenza** della ricetta.
- Le ricette si imparano leggendo in Biblioteca, facendosi insegnare da un NPC con abbastanza affinità (Fase 4), o sperimentando: combinare oggetti a caso e scoprire (come in Minecraft).

### 3.3 UI di crafting [G]
- L'inventario del giocatore ha **slot** come in The Escapists: 8 slot, i materiali si impilano fino a 10, più uno slot per l'abito e uno per l'attrezzo in mano.
- **Finestra "Crafting"** (tasto **C**): si trascinano o si scelgono fino a 3 oggetti dall'inventario e si vede l'anteprima del risultato se la combinazione è valida, altrimenti "?". Accanto c'è l'elenco delle ricette conosciute, con quelle eseguibili evidenziate.
- Vicino a un banco, con E si apre la stessa finestra con le ricette di quel banco.
- Il crafting richiede tempo di gioco: il personaggio lavora al banco con un'animazione.
- **[T]** Craftare consuma esattamente gli ingredienti; senza banco o conoscenza la ricetta fallisce; l'inventario pieno blocca il risultato senza perdere gli ingredienti.

---

## Fase 4: il giocatore come personaggio

> **Fatte 4.1 e 4.2 (2026-09-29).** `World.player: PlayerCharacter` (`sim/src/player.rs`, API in `world/player.rs`) con nome, posto (carrozza + piano, sincronizzato dal game ogni frame con `set_player_place`), cabina, inventario a slot, baule, gettoni, ricette imparate; `job` e `needs` sono riservati (sopravvivenza spenta).
> - **Gettoni nella sim:** la moneta si conserva sempre, anche con acquisti, vendite, prelievi, crafting e regali del giocatore (`money_supply` include il giocatore). `player_buy/sell/take/give/craft` usano `world.player`; il game non ha più `PlayerInventory`.
> - **`SlotInventory`:** 12 scomparti (baule 24), una pila per scomparto; il limite di pila viene dal catalogo (`ItemDef::stack_limit`, default `DEFAULT_STACK_LIMIT` = 10; 5 per cibo e bevande, 1 per i beni durevoli). Nessun `match` sugli oggetti né array per `ItemKind`: pronto per un catalogo che cresce.
> - **Relazioni:** `Npc::player: Option<PlayerTie>` (affinità, ultimo saluto, "ha qualcosa da dirti"), separato dalle relazioni tra NPC per non toccare conversazioni e deliberazioni. Regali +, acquisti al suo bancone + poco, furti visti −. Effetti: ±10% sul prezzo al bancone, chi diffida rifiuta i regali, chi ti vuole bene ti saluta (fumetto) e si avvicina; un amico che ti ha salutato mostra un "!" (aggancio per la chat 5.4).
> - **Cabina:** un letto privato (`Station::owner = Player`, mai usato né contato dagli NPC) al piano di sopra del primo Dormitorio, con paravento e baule. E sul letto dalle 20:00: il tempo corre fino alle 06:00 e la partita si salva; E sul baule apre la finestra di trasferimento. Non ci sono ancora stanze vere, la carrozza Cabine, la mensola, l'arredamento né le visite degli amici.
> - Salvataggi: `SAVE_VERSION` 8, il corpo contiene solo la posizione fisica del giocatore. Il nome si sceglie in "Nuova partita".

### 4.1 Il giocatore nella sim [S]
- Nuovo `World.player: PlayerCharacter`, salvato con il mondo:
  ```rust
  pub struct PlayerCharacter {
      pub name: String,
      pub place: Place,             // sincronizzato dal game ogni tick
      pub home: RoomId,             // la sua cabina
      pub inventory: SlotInventory, // sostituisce PlayerInventory del game
      pub tokens: u32,
      pub known_recipes: Vec<RecipeId>,
      pub skills: Skills,
      pub job: Option<Job>,
      pub needs: Option<Needs>,     // solo in modalità "sopravvivenza"
  }
  ```
- Gli NPC hanno relazioni **anche con il giocatore** (`Relation { other: Person::Player, .. }`). L'affinità cambia i prezzi, le risposte in chat, i favori e gli incarichi.
- Il giocatore conta nella presenza: gli NPC possono avvicinarsi e rivolgergli la parola.
- La fisica e la posizione esatta restano al `game` (il platform in `player.rs`), che comunica alla sim solo il `place`.

### 4.2 La stanza del giocatore
- Una cabina nella carrozza Cabine, con letto (dormire fa passare il tempo e salva la partita), baule (magazzino personale, `storage.rs`) e mensola degli attrezzi.
- Facoltativo: arredarla con oggetti craftati. Si può anche cambiare stanza comprandone o affittandone un'altra.
- Gli NPC non entrano nelle stanze altrui senza permesso. Gli amici stretti possono venire a trovarti.

### 4.3 Attività del giocatore
- **Lavorare:** chiedere un lavoro al capo carrozza, fare il turno alla postazione (un minigioco semplice o un'azione a tempo) e ricevere il salario. È lo stesso sistema degli NPC.
- **Incarichi e favori** (The Escapists): gli NPC chiedono "portami 3 Verdure", "ripara la mia lampada", "consegna questo pacco". Ricompensa: gettoni, affinità, ricette.
- **Scambi:** comprare, vendere (`player_sell`), barattare direttamente con gli NPC.
- **Espedienti:** rubare, con il rischio che le Guardie ti vedano (affinità in calo, multa, una notte in cella). Facoltativo, dipende dal tono del gioco.

---

## Fase 5: dialoghi e chat

### 5.1 Conversazioni tra NPC [S]
- `Socialize` diventa a due: il partner, se disponibile, passa a `Socialize(A)` con la stessa scadenza.
- Nuovo registro `World.conversations` (dimensione limitata) con `Conversation { a, b, place, since, until, topic, outcome }`.
- Argomenti (`Topic`): bisogni, lavoro, eventi recenti (nascite, morti, guasti, carenze), relazioni, **prezzi** (passaparola: Fase 6), **il giocatore** ("hai visto cos'ha fatto quello della cabina 12?").
- Le conversazioni hanno effetti: affinità, informazioni sui prezzi, ricette insegnate, voci sul giocatore.

### 5.2 Testo [S]
- Nuovo modulo `sim/src/dialogue.rs`: frasi a modelli in italiano, con varianti deterministiche e un tono che dipende dal carattere dell'NPC. Per dare tono alle frasi, ogni NPC riceve 2–3 **tratti** di personalità (burbero, allegro, pettegolo, timido…).

### 5.3 Fumetti [G]
- Nuovo `speech.rs`, sullo schema di `chat_hearts`: `Text2d` su uno sfondo pixel-art a 9 riquadri sopra la testa, righe alternate ogni 2,5 s reali, al massimo circa 4 fumetti a schermo. A velocità alte si vede solo "…" o "!".

### 5.4 Chat del giocatore [G+S]

> **Fatta (2026-09-29).** Tipi in `sim/src/chat.rs` (`Intent`, `Band`, `ChatLog`, `Favour`, `ChatReply`), API in `world/chat.rs` (`player_chat_start`, `player_chat`, `player_chat_text`, `player_chat_gift`, `chat_log`, `player_favour`), testi in `dialogue/chat.rs`.
> - **Intenzioni:** Saluta, Chiedi del lavoro, Chiedi dei prezzi (i Mercati entro 6 carrozze da casa, lavoro o dove si trova; i mercanti li conoscono tutti), Chiedi un favore, Regala, Scambia, Chiedi notizie (le notizie delle conversazioni e i pettegolezzi sul giocatore), Insulta, Congedati. Risposte per intenzione × affinità (bassa / media / alta) × tratti × stato (fame, stanchezza, turno, età).
> - **Testo libero:** `KeywordReader` (parole, radici e frasi; accenti, maiuscole e lettere allungate ignorati; un errore di battitura da 5 lettere, due da 8), dietro il trait `IntentReader`, così un lettore LLM potrà sostituirlo. Se non capisce, l'NPC dice di non aver capito e non succede nient'altro. Un oggetto nominato ("quanto costa un vestito?") guida la risposta sui prezzi.
> - **Effetti:** saluto +0,03 (al massimo ogni 6 ore), insulto −0,15 (al massimo ogni ora), incarico fatto +0,15. Gli incarichi ("portami due barre di metallo") nascono dal catalogo (un bene che manca, cibo se ha fame, un ingrediente del suo lavoro che scarseggia), durano 2 giorni e si pagano con i gettoni dell'NPC (fino a metà dei suoi), al prezzo del Mercato più economico: la moneta si conserva. Chi ti saluta può pensare a un incarico e mostrare il "!": aprendo la chat parla per primo.
> - **Memoria:** le ultime 24 righe per NPC in `PlayerCharacter::chats` (96 NPC al massimo). `SAVE_VERSION` 9.
> - **Gioco (`chat.rs`):** T (o E se non c'è niente da regalare) apre la finestra con ritratto, lavoro, "Ti considera: …", incarico, storico, risposte suggerite e campo di testo (Invio invia, Esc chiude). **Il tempo si ferma** mentre si parla (la chat mette la pausa e la toglie alla chiusura, se l'aveva messa lei; P e 1–5 restano attivi) e il giocatore resta fermo. Le battute appaiono anche come fumetti sopra l'NPC e il giocatore; "Regala" apre la scelta del regalo, "Scambia" con un mercante al banco la finestra del Mercato.
> - Non ancora: incarichi diversi da "portami X" (riparazioni, consegne), ricette insegnate in chat, un lettore LLM.

- Avvicinandosi a un NPC e premendo **T** (o E → "Parla") si apre la **finestra di chat**: lo storico dei messaggi con quel personaggio, con il suo ritratto e l'affinità.
- Il giocatore può:
  - scegliere tra **risposte suggerite** (saluta, chiedi del lavoro, chiedi i prezzi, chiedi un favore, regala, scambia, insulta…): funzionano sempre;
  - **scrivere a testo libero**: il testo viene capito (vedi decisione più sotto) e ricondotto a una di queste intenzioni.
- Le risposte vengono da `dialogue.rs` in base a intenzione, stato dell'NPC, affinità e tratti. Le azioni ("Scambia", "Accetta incarico") aprono direttamente la finestra giusta.
- Anche l'NPC può iniziare: se ha un'alta affinità con te o un incarico da proporti, ti ferma e ti appare un fumetto con "!".
- **[T]** Ogni intenzione ha una risposta per ogni combinazione di affinità (bassa, media, alta); la chat non cambia il determinismo della sim (gli effetti passano da API `player_*` come oggi).

---

## Fase 6: economia

L'impianto della versione 1 resta, adattato al catalogo nuovo:

- **6a. Prezzo per distanza:** `prezzo = (base + trasporto_per_carrozza × distanza_dal_produttore) × (1 + scarcity_markup × (1 − riempimento))`. **[T]** A stock uguale, il prezzo cresce con la distanza.
- **6b. Specialità per carrozza:** alla generazione ogni carrozza produttrice riceve 1–2 specialità tra le ricette del suo tipo. La Serra 2 fa Verdura, la Serra 12 Tè e Cotone: la geografia del treno conta.
- **6c. Merce trasportata a piedi:** i prelievi istantanei diventano `Action::Haul` con `npc.cargo`, eseguiti da Facchini e Mercanti che si vedono camminare con una cassa. **[T]** La merce si conserva.
- **6d. Denaro che circola** (già in gran parte fatto dall'altra sessione, con una tesoreria centrale al posto delle casse per carrozza): casse (`till`) per carrozze e mercati. I salari escono dalle casse, gli acquisti le riempiono, e l'unica entrata esterna è la tesoreria del treno. Il pasto in Mensa ha un prezzo. Tutto dietro l'interruttore `SimParams::closed_economy` finché non è bilanciato. **[T]** I gettoni si conservano.
- **6e. Decisioni guidate dai prezzi:** ogni NPC ha i suoi `known_prices` (dalle visite e dal passaparola) e sceglie tra il Mercato vicino e quello conveniente.
- **6f. UI:** finestra **"Mercato"** (tasto **M**) con la lista degli oggetti, prezzo, stock, produttore, distanza, tendenza e il pulsante Compra/Vendi; finestra **"Listino"** con la tabella oggetti × mercati colorata da economico a caro; storico dei prezzi nel `game` con un grafico nella finestra Popolazione (tasto G).

---

## Ordine e dipendenze

```
Fase 0 (definizioni) ──► Fase 1 (stanze, piani, movimento) ──► Fase 2 (carrozze, lavori)
        │                                                              │
        └──► Fase 3 (crafting) ──► Fase 4 (giocatore) ──► Fase 5 (dialoghi, chat)
                                                               │
                                     Fase 6 (economia) ◄───────┘ (6e usa il passaparola)
```

Traguardi giocabili, in ordine:

1. **"Si cammina":** Fase 0 e Fase 1 con i tipi di carrozza attuali, di cui uno a due piani.
2. **"Si crafta":** Fase 3 con 10–15 oggetti e la finestra C; il giocatore ha l'inventario a slot.
3. **"Casa mia":** Fase 4.1–4.2, con la cabina, il baule e gli NPC che ti riconoscono.
4. **"Si parla":** Fase 5, prima i fumetti e poi la chat con le risposte suggerite.
5. **"Il treno lavora":** Fase 2 con i tipi nuovi, a gruppi, e la gestione.
6. **"Il mercato":** Fase 6a + 6f, poi 6b → 6c → 6e → 6d.

Ogni traguardo: `cargo test -p sim`, 30 giorni di `examples/headless.rs` senza morie, poi una prova nel gioco.

---

## Decisioni aperte

1. **Come capire il testo libero in chat.** Proposta: **risposte suggerite sempre disponibili + testo libero interpretato da Laya** (`sim-laya` c'è già e fa proprio questo: classificare una frase in una lista di intenzioni). Le risposte restano a modelli. Alternativa: un LLM generativo, locale o via API, che scrive anche le risposte: più varietà, ma latenza, costo e niente determinismo. Si può sempre aggiungere dopo, come "voce" che riformula la frase del modello.
2. **Bisogni del giocatore (modalità sopravvivenza).** Deve mangiare e dormire come gli NPC? Proposta: sì, ma disattivabile.
3. **Furti e guardie.** Il tono è da The Escapists (espedienti, guardie, cella) o più tranquillo? Cambia la presenza del Posto di guardia e degli oggetti "illegali".
4. **Due piani e la camera.** Due piani da 96 unità con la camera che segue il piano, oppure la camera che si allarga fino a mostrare tutta la carrozza? Da decidere con un prototipo.
5. **Velocità di camminata:** 2 o 3 minuti di gioco per carrozza, da provare a 1×.
