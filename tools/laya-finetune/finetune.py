#!/usr/bin/env python3
"""Fine-tuning di laya-multilingual sulle decisioni di TrainGame.

Legge il dataset di `cargo run -p sim-laya --release --example export_dataset`
(train/val/test.jsonl), mette a punto `convaiinnovations/laya-multilingual` con la
ricetta RLCD del notebook ufficiale di Laya (soft cross-entropy sulla distribuzione
dell'insegnante + policy gradient su logit rumorosi), fitta le temperature per
(tipo, numero di opzioni) sulla validazione, valuta sul test e salva una checkpoint
con lo stesso layout del repository Hugging Face, quello che `LayaModel::load`
(`LAYA_MODEL_DIR`, `laya_eval --model-dir`) si aspetta:

    model.safetensors  encoder/config.json  rl_agent_config.json
    tokenizer/tokenizer.json  tokenizer/tokenizer_config.json

Riferimenti upstream (Laya, Apache-2.0), commit 9d955671415fc19f069b9cc998928075c1f255ec:
- notebooks/laya_finetune_typed_decisions_2xT4_kaggle.ipynb (celle 6 e 8: preprocessing,
  train_ddp.py con RLCD, fit delle temperature, salvataggio);
- docs/finetune.md; laya/common.py (build_model, build_sequence, proper_reward, ece_score);
- PyPI `laya==0.3.21` (è la versione di quel commit).

Uso (vedi README.md):

    python finetune.py --data DIR --check-data             # solo stdlib: valida e conta
    python finetune.py --data DIR --out laya-traingame-ft  # GPU: allena, calibra, valuta, salva
    python finetune.py --data DIR --eval-only CHECKPOINT   # valuta una checkpoint sul test
    python finetune.py --check-checkpoint DIR [--base DIR] # solo stdlib: controlla il layout

Le parti senza torch (lettura del dataset, fit delle temperature, ECE, controllo della
checkpoint) usano solo la libreria standard e sono coperte da test_finetune_data.py.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import random
import shutil
import struct
import sys
import time
from dataclasses import dataclass, field
from typing import Any, Dict, Iterable, List, Optional, Sequence, Tuple

# --- Costanti (tenute allineate con il codice Rust) -------------------------------------

BASE_REPO = "convaiinnovations/laya-multilingual"
# `LAYA_MULTILINGUAL_REVISION` in crates/sim-laya/src/laya/download.rs, e lo SHA "reviewed"
# di `laya/revisions.py::PINNED_REVISIONS` upstream.
BASE_REVISION = "e4e9ddf21a7b1903b7acffd8814ad4307bf63a67"
UPSTREAM_COMMIT = "9d955671415fc19f069b9cc998928075c1f255ec"
QUESTION_ID = "decision"
SPLITS = ("train", "val", "test")
# I file che `CheckpointFiles::in_dir` (download.rs) richiede.
CHECKPOINT_FILES = (
    "encoder/config.json",
    "rl_agent_config.json",
    "tokenizer/tokenizer.json",
    "tokenizer/tokenizer_config.json",
    "model.safetensors",
)
# Limiti delle temperature: `clamp_temperature` sia in Laya sia in sequence.rs.
TEMP_MIN, TEMP_MAX = 0.5, 5.0
# Seme e dimensione della fetta di calibrazione quando manca lo split val (come il notebook).
CALIB_SEED, CALIB_MAX = 20260922, 400


# --- Dataset (solo stdlib) ------------------------------------------------------------------

@dataclass
class Row:
    """Una domanda `choice` del dataset, pronta per il modello."""

    id: str
    kind: str
    source: str
    case: str
    split: str
    state: Any
    instructions: str
    # `crit` nel formato interno di Laya (`Agent._to_internal`): una lista di descrizioni
    # diventa {descrizione: None} e si rende senza lettere, come `OptionLabels::Plain` in Rust.
    crit: Dict[str, Any]
    keys: List[str]
    target: List[float]
    label: int
    tags: List[str] = field(default_factory=list)

    @property
    def k(self) -> int:
        return len(self.keys)


def _maybe_json(value: Any) -> Any:
    """`typed-decisions` su HF salva state/questions/gold come stringhe JSON: le accetta."""
    if isinstance(value, str):
        s = value.strip()
        if s[:1] in "{[":
            try:
                return json.loads(s)
            except ValueError:
                return value
    return value


def parse_row(raw: Dict[str, Any], split: Optional[str] = None) -> Row:
    """Valida una riga JSONL (schema di export_dataset, o un caso typed-decisions con una
    sola domanda choice) e la normalizza. Solleva ValueError con un messaggio chiaro."""
    rid = str(raw.get("id", "?"))
    questions = _maybe_json(raw.get("questions"))
    gold = _maybe_json(raw.get("gold"))
    if not isinstance(questions, dict) or not questions:
        raise ValueError(f"{rid}: 'questions' mancante o vuoto")
    qid = QUESTION_ID if QUESTION_ID in questions else next(iter(questions))
    q = questions[qid]
    if q.get("type") != "choice":
        raise ValueError(f"{rid}: solo domande 'choice' (trovato {q.get('type')!r})")
    crit_raw = q.get("criteria")
    if isinstance(crit_raw, list):
        keys = [str(c) for c in crit_raw]
        crit = {c: None for c in keys}
    elif isinstance(crit_raw, dict):
        keys = [str(c) for c in crit_raw]
        crit = {str(k): v for k, v in crit_raw.items()}
    else:
        raise ValueError(f"{rid}: 'criteria' deve essere una lista o un dizionario")
    if len(keys) < 2:
        raise ValueError(f"{rid}: servono almeno 2 opzioni")
    if len(set(keys)) != len(keys):
        raise ValueError(f"{rid}: opzioni ripetute")
    if not isinstance(gold, dict) or qid not in gold:
        raise ValueError(f"{rid}: 'gold' non ha la domanda {qid!r}")
    g = gold[qid]
    probs = g.get("probabilities") or {}
    target = [float(probs.get(k, 0.0)) for k in keys]
    if any(not math.isfinite(p) or p < 0 for p in target):
        raise ValueError(f"{rid}: probabilità non valide")
    s = sum(target)
    label_key = g.get("label")
    if "label_index" in g:
        label = int(g["label_index"])
    elif label_key is not None and str(label_key) in keys:
        label = keys.index(str(label_key))
    elif s > 0:
        label = max(range(len(keys)), key=lambda i: target[i])
    else:
        raise ValueError(f"{rid}: né etichetta né probabilità")
    if not 0 <= label < len(keys):
        raise ValueError(f"{rid}: label_index {label} fuori dalle {len(keys)} opzioni")
    if label_key is not None and str(label_key) != keys[label]:
        raise ValueError(f"{rid}: 'label' e 'label_index' non coincidono")
    target = [p / s for p in target] if s > 0 else [float(i == label) for i in range(len(keys))]
    state = _maybe_json(raw.get("state"))
    if state in (None, ""):
        raise ValueError(f"{rid}: 'state' vuoto")
    tags = raw.get("option_tags") or []
    return Row(
        id=rid,
        kind=str(raw.get("kind", "choice")),
        source=str(raw.get("source", "?")),
        case=str(raw.get("case", "")),
        split=str(split or raw.get("split", "train")),
        state=state,
        instructions=str(q.get("instructions", "")),
        crit=crit,
        keys=keys,
        target=target,
        label=label,
        tags=[str(t) for t in tags] if len(tags) == len(keys) else [],
    )


def load_jsonl(path: str, split: Optional[str] = None) -> List[Row]:
    rows = []
    with open(path, encoding="utf-8") as f:
        for n, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                rows.append(parse_row(json.loads(line), split))
            except ValueError as e:
                raise ValueError(f"{path}:{n}: {e}") from None
    return rows


def load_dataset_dir(data_dir: str) -> Dict[str, List[Row]]:
    """train/val/test.jsonl di una cartella (gli split mancanti sono vuoti)."""
    out = {}
    for split in SPLITS:
        path = os.path.join(data_dir, f"{split}.jsonl")
        out[split] = load_jsonl(path, split) if os.path.exists(path) else []
    if not out["train"] and not out["test"]:
        raise ValueError(f"{data_dir}: né train.jsonl né test.jsonl")
    return out


def check_no_leakage(data: Dict[str, List[Row]]) -> None:
    """Nessun id ripetuto tra split, e (se le righe hanno `seed`) nessun seme in due split."""
    seen: Dict[str, str] = {}
    for split, rows in data.items():
        for r in rows:
            if r.id in seen and seen[r.id] != split:
                raise ValueError(f"riga {r.id} sia in {seen[r.id]} sia in {split}")
            seen[r.id] = split


def dataset_stats(rows: Sequence[Row]) -> Dict[str, Any]:
    by: Dict[str, int] = {}
    positions: Dict[int, int] = {}
    labels: Dict[str, int] = {}
    for r in rows:
        key = f"{r.kind}/{r.source}"
        by[key] = by.get(key, 0) + 1
        positions[r.label] = positions.get(r.label, 0) + 1
        if r.tags:
            t = f"{r.kind}:{r.tags[r.label]}"
            labels[t] = labels.get(t, 0) + 1
    n = max(1, len(rows))
    return {
        "rows": len(rows),
        "by_kind_source": dict(sorted(by.items())),
        "label_position_share": {p: round(c / n, 3) for p, c in sorted(positions.items())},
        "label_tags": dict(sorted(labels.items())),
        "teacher_confidence": round(sum(max(r.target) for r in rows) / n, 3),
    }


def split_calibration(rows: Sequence[Row], seed: int = CALIB_SEED, max_items: int = CALIB_MAX,
                      fraction: float = 0.1) -> Tuple[List[Row], List[Row]]:
    """Come il notebook: una fetta fissa (≤ 400 o 10%) tolta dal train prima di allenare,
    per fittare le temperature su righe mai viste. Restituisce (train, calibrazione)."""
    order = list(range(len(rows)))
    random.Random(seed).shuffle(order)
    n = min(max_items, int(len(rows) * fraction))
    calib = set(order[:n])
    return ([r for i, r in enumerate(rows) if i not in calib],
            [r for i, r in enumerate(rows) if i in calib])


# --- Temperature, metriche (solo stdlib) -----------------------------------------------------

def temperature_bucket(k: int, qtype: str = "choice") -> str:
    """`temp_bucket` di laya/common.py e `temperature_bucket` di sequence.rs."""
    size = "2" if k <= 2 else "3-5" if k <= 5 else "6-10" if k <= 10 else "11+"
    return f"{qtype}:{size}"


def clamp_temperature(t: float) -> float:
    try:
        t = float(t)
    except (TypeError, ValueError):
        return 1.0
    if not math.isfinite(t):
        return 1.0
    return min(TEMP_MAX, max(TEMP_MIN, t))


def softmax(logits: Sequence[float], t: float = 1.0) -> List[float]:
    z = [x / t for x in logits]
    m = max(z)
    e = [math.exp(v - m) for v in z]
    s = sum(e)
    return [v / s for v in e]


def soft_nll(pairs: Iterable[Tuple[Sequence[float], Sequence[float]]], t: float) -> float:
    """Cross-entropy media tra la distribuzione dell'insegnante e softmax(logit / t)."""
    total, n = 0.0, 0
    for logits, target in pairs:
        p = softmax(logits, t)
        total -= sum(g * math.log(max(q, 1e-12)) for g, q in zip(target, p) if g > 0)
        n += 1
    return total / max(1, n)


def fit_temperature(pairs: Sequence[Tuple[Sequence[float], Sequence[float]]],
                    lo: float = TEMP_MIN, hi: float = TEMP_MAX, iters: int = 60) -> float:
    """Temperatura che minimizza `soft_nll`, cercata (sezione aurea) su log t in [lo, hi]:
    lo stesso obiettivo del notebook (LBFGS su log t), già nei limiti che Laya e Rust
    applicano. Con meno di 10 righe restituisce 1.0, come il notebook."""
    if len(pairs) < 10:
        return 1.0
    a, b = math.log(lo), math.log(hi)
    phi = (math.sqrt(5) - 1) / 2
    c, d = b - phi * (b - a), a + phi * (b - a)
    fc, fd = soft_nll(pairs, math.exp(c)), soft_nll(pairs, math.exp(d))
    for _ in range(iters):
        if fc < fd:
            b, d, fd = d, c, fc
            c = b - phi * (b - a)
            fc = soft_nll(pairs, math.exp(c))
        else:
            a, c, fc = c, d, fd
            d = a + phi * (b - a)
            fd = soft_nll(pairs, math.exp(d))
    return clamp_temperature(math.exp((a + b) / 2))


def fit_temperatures(rows: Sequence[Row], logits: Sequence[Sequence[float]],
                     min_items: int = 10) -> Dict[str, Any]:
    """Una temperatura per tutto il tipo `choice` e una per bucket di opzioni
    ("choice:2", "choice:3-5", …) con almeno `min_items` righe; per diagnostica anche una
    per tipo di riga (azione/deliberazione), che il runtime non sa distinguere."""
    pairs = [(lg, r.target) for r, lg in zip(rows, logits)]
    out: Dict[str, Any] = {"choice": fit_temperature(pairs), "by_options": {}, "by_kind": {},
                           "count": len(pairs)}
    buckets: Dict[str, list] = {}
    kinds: Dict[str, list] = {}
    for r, pair in zip(rows, pairs):
        buckets.setdefault(temperature_bucket(r.k), []).append(pair)
        kinds.setdefault(r.kind, []).append(pair)
    for b, ps in sorted(buckets.items()):
        if len(ps) >= min_items:
            out["by_options"][b] = round(fit_temperature(ps), 4)
    for kname, ps in sorted(kinds.items()):
        if len(ps) >= min_items:
            out["by_kind"][kname] = round(fit_temperature(ps), 4)
    out["choice"] = round(out["choice"], 4)
    return out


def temperature_for(temps: Dict[str, Any], k: int) -> float:
    """Come `AgentConfig::temperature` in Rust: prima il bucket, poi il tipo."""
    t = temps.get("temperature_by_options", {}).get(temperature_bucket(k))
    if t is None:
        t = temps.get("temperature", [1.0, 1.0, 1.0])[0]
    return clamp_temperature(t)


def ece_score(conf: Sequence[float], correct: Sequence[float], bins: int = 15) -> float:
    """Expected Calibration Error, come `ece_score` di laya/common.py."""
    if not conf:
        return float("nan")
    n = len(conf)
    e = 0.0
    for i in range(bins):
        lo, hi = i / bins, (i + 1) / bins
        sel = [j for j, c in enumerate(conf) if (c >= lo if i == 0 else c > lo) and c <= hi]
        if sel:
            mc = sum(conf[j] for j in sel) / len(sel)
            ma = sum(correct[j] for j in sel) / len(sel)
            e += len(sel) / n * abs(mc - ma)
    return e


def evaluate(rows: Sequence[Row], logits: Sequence[Sequence[float]],
             temps: Dict[str, Any]) -> Dict[str, Any]:
    """Accuratezza rispetto all'etichetta, accuratezza "morbida", NLL, Brier ed ECE,
    in totale e per gruppo (tipo, origine, caso ovvio, posizione dell'etichetta)."""
    per_row = []
    for r, lg in zip(rows, logits):
        p = softmax(lg, temperature_for(temps, r.k))
        pred = max(range(len(p)), key=lambda i: p[i])
        per_row.append({
            "correct": float(pred == r.label),
            "conf": p[pred],
            "soft": sum(a * b for a, b in zip(p, r.target)),
            "nll": -sum(g * math.log(max(q, 1e-12)) for g, q in zip(r.target, p) if g > 0),
            "brier": sum((a - b) ** 2 for a, b in zip(p, r.target)),
        })

    def summary(idx: List[int]) -> Dict[str, float]:
        if not idx:
            return {"n": 0}
        m = lambda key: sum(per_row[i][key] for i in idx) / len(idx)  # noqa: E731
        return {
            "n": len(idx),
            "accuracy": round(m("correct"), 4),
            "soft_accuracy": round(m("soft"), 4),
            "mean_confidence": round(m("conf"), 4),
            "nll": round(m("nll"), 4),
            "brier": round(m("brier"), 4),
            "ece": round(ece_score([per_row[i]["conf"] for i in idx],
                                   [per_row[i]["correct"] for i in idx]), 4),
        }

    groups: Dict[str, Dict[str, List[int]]] = {"kind": {}, "kind_source": {}, "case": {},
                                               "label_position": {}}
    for i, r in enumerate(rows):
        groups["kind"].setdefault(r.kind, []).append(i)
        groups["kind_source"].setdefault(f"{r.kind}/{r.source}", []).append(i)
        if r.source == "obvious":
            groups["case"].setdefault(r.case, []).append(i)
        groups["label_position"].setdefault(str(r.label), []).append(i)
    return {
        "all": summary(list(range(len(rows)))),
        **{g: {k: summary(v) for k, v in sorted(d.items())} for g, d in groups.items()},
    }


def format_report(name: str, rep: Dict[str, Any]) -> str:
    def line(label: str, s: Dict[str, Any]) -> str:
        if not s.get("n"):
            return f"  {label:<48} —"
        return (f"  {label:<48} n={s['n']:<5} acc {s['accuracy']:.1%}  soft {s['soft_accuracy']:.2f}"
                f"  conf {s['mean_confidence']:.2f}  ECE {s['ece']:.3f}  NLL {s['nll']:.3f}")
    out = [f"{name}", line("tutto", rep["all"])]
    for group in ("kind", "kind_source", "case", "label_position"):
        for k, s in rep.get(group, {}).items():
            out.append(line(f"{group}={k}", s))
    return "\n".join(out)


# --- Checkpoint (solo stdlib) --------------------------------------------------------------

def safetensors_header(path: str) -> Dict[str, Any]:
    """Intestazione di un file safetensors (nomi, dtype, forme) senza leggere i pesi."""
    with open(path, "rb") as f:
        (n,) = struct.unpack("<Q", f.read(8))
        if n <= 0 or n > 100_000_000:
            raise ValueError(f"{path}: intestazione safetensors non valida")
        header = json.loads(f.read(n))
    header.pop("__metadata__", None)
    return header


def check_checkpoint(ckpt: str, base: Optional[str] = None) -> List[str]:
    """Problemi della checkpoint rispetto a quello che `LayaModel::load` legge (lista vuota =
    a posto). Con `base`, confronta anche nomi e forme dei tensori con la checkpoint base."""
    problems = []
    for f in CHECKPOINT_FILES:
        if not os.path.isfile(os.path.join(ckpt, f)):
            problems.append(f"manca {f}")
    if problems:
        return problems
    try:
        with open(os.path.join(ckpt, "encoder", "config.json"), encoding="utf-8") as f:
            enc = json.load(f)
        if enc.get("model_type") != "modernbert":
            problems.append(f"encoder/config.json: model_type {enc.get('model_type')!r}")
    except ValueError as e:
        problems.append(f"encoder/config.json: {e}")
    try:
        with open(os.path.join(ckpt, "rl_agent_config.json"), encoding="utf-8") as f:
            cfg = json.load(f)
        t = cfg.get("temperature", [1.0, 1.0, 1.0])
        if not (isinstance(t, list) and len(t) == 3):
            problems.append("rl_agent_config.json: 'temperature' deve essere una lista di 3")
        for k, v in (cfg.get("temperature_by_options") or {}).items():
            if clamp_temperature(v) != v:
                problems.append(f"rl_agent_config.json: {k}={v} fuori da [{TEMP_MIN}, {TEMP_MAX}]")
    except ValueError as e:
        problems.append(f"rl_agent_config.json: {e}")
    try:
        with open(os.path.join(ckpt, "tokenizer", "tokenizer_config.json"), encoding="utf-8") as f:
            tcfg = json.load(f)
        for key in ("cls_token", "sep_token", "mask_token", "pad_token"):
            if key not in tcfg:
                problems.append(f"tokenizer_config.json: manca {key}")
    except ValueError as e:
        problems.append(f"tokenizer_config.json: {e}")
    try:
        header = safetensors_header(os.path.join(ckpt, "model.safetensors"))
        if not any(k.startswith("encoder.") for k in header):
            problems.append("model.safetensors: nessun tensore encoder.*")
        for k in ("type_emb.weight", "scorer.0.weight", "scorer.3.weight"):
            if k not in header:
                problems.append(f"model.safetensors: manca {k}")
        if base:
            ref = safetensors_header(os.path.join(base, "model.safetensors"))
            missing = sorted(set(ref) - set(header))
            extra = sorted(set(header) - set(ref))
            if missing:
                problems.append(f"model.safetensors: mancano {missing[:5]}…")
            if extra:
                problems.append(f"model.safetensors: in più {extra[:5]}…")
            for k in set(ref) & set(header):
                if ref[k]["shape"] != header[k]["shape"]:
                    problems.append(f"model.safetensors: forma di {k} diversa")
    except (OSError, ValueError, KeyError) as e:
        problems.append(f"model.safetensors: {e}")
    return problems


# --- Torch: modello, dati, training ---------------------------------------------------------

def _torch():
    try:
        import torch  # noqa: F401
        import laya  # noqa: F401
    except ImportError as e:
        sys.exit(f"serve PyTorch + laya ({e}); pip install -r requirements.txt")
    import torch
    return torch


def resolve_base(base: str, revision: str) -> str:
    """Cartella della checkpoint base: locale, o scaricata da HF al commit fissato."""
    if os.path.isdir(base):
        return base
    from huggingface_hub import snapshot_download
    return snapshot_download(base, revision=revision, allow_patterns=[
        "model.safetensors", "rl_agent_config.json", "encoder/*", "tokenizer/*"])


def load_model(model_dir: str, device):
    """Tokenizer, configurazione e modello di una checkpoint Laya (come il notebook)."""
    from safetensors.torch import load_file
    from transformers import AutoTokenizer
    from laya.agent import _fix_tokenizer_config
    from laya.common import build_model

    _fix_tokenizer_config(model_dir)
    tok = AutoTokenizer.from_pretrained(os.path.join(model_dir, "tokenizer"))
    with open(os.path.join(model_dir, "rl_agent_config.json"), encoding="utf-8") as f:
        cfg = json.load(f)
    model = build_model(cfg, encoder_dir=os.path.join(model_dir, "encoder"))
    model.load_state_dict(load_file(os.path.join(model_dir, "model.safetensors")), strict=True)
    return tok, cfg, model.to(device)


def build_items(rows: Sequence[Row], tok, max_len: int, head_max_len: int,
                targets: str = "soft") -> Tuple[List[Dict[str, Any]], List[Row], int]:
    """Righe → sequenze di token (`build_sequence` di Laya, come a inferenza). Le righe
    in cui qualche opzione perde il proprio [MASK] si scartano (e si contano)."""
    from laya.common import QTYPES, build_sequence

    items, kept, dropped = [], [], 0
    for r in rows:
        q = {"t": "choice", "ins": r.instructions, "crit": r.crit}
        ids, markers = build_sequence(tok, r.state, q, max_len, head_max_len)
        if len(markers) != r.k:
            dropped += 1
            continue
        target = r.target if targets == "soft" else [float(i == r.label) for i in range(r.k)]
        items.append({"ids": ids, "markers": markers, "qtype": QTYPES["choice"],
                      "target": target, "label": r.label})
        kept.append(r)
    return items, kept, dropped


def collate(items: Sequence[Dict[str, Any]], pad_id: int):
    """`collate_train_batch` del notebook."""
    torch = _torch()
    n, L = len(items), max(len(it["ids"]) for it in items)
    kmax = max(len(it["markers"]) for it in items)
    ids = torch.full((n, L), pad_id, dtype=torch.long)
    att = torch.zeros((n, L), dtype=torch.long)
    mpos = torch.zeros((n, kmax), dtype=torch.long)
    mmask = torch.zeros((n, kmax), dtype=torch.bool)
    target = torch.zeros((n, kmax), dtype=torch.float32)
    for i, it in enumerate(items):
        ids[i, : len(it["ids"])] = torch.tensor(it["ids"])
        att[i, : len(it["ids"])] = 1
        k = len(it["markers"])
        mpos[i, :k] = torch.tensor(it["markers"])
        mmask[i, :k] = True
        target[i, :k] = torch.tensor(it["target"], dtype=torch.float32)
    return {"input_ids": ids, "attention_mask": att, "marker_pos": mpos, "marker_mask": mmask,
            "target": target, "qtype": torch.tensor([it["qtype"] for it in items]),
            "label": torch.tensor([it["label"] for it in items])}


def amp_dtype(torch, device):
    if device.type != "cuda":
        return None
    return torch.bfloat16 if torch.cuda.is_bf16_supported() else torch.float16


def predict_logits(model, items, pad_id, device, batch: int = 32) -> List[List[float]]:
    """Logit (senza temperatura) per ogni item, a lotti ordinati per lunghezza."""
    torch = _torch()
    model.eval()
    order = sorted(range(len(items)), key=lambda i: len(items[i]["ids"]))
    out: List[Optional[List[float]]] = [None] * len(items)
    dt = amp_dtype(torch, device)
    with torch.no_grad():
        for s in range(0, len(order), batch):
            idx = order[s:s + batch]
            b = collate([items[i] for i in idx], pad_id)
            with torch.autocast(device.type, dtype=dt, enabled=dt is not None):
                logits, _ = model(b["input_ids"].to(device), b["attention_mask"].to(device),
                                  b["marker_pos"].to(device), b["marker_mask"].to(device),
                                  b["qtype"].to(device))
            logits = logits.float().cpu()
            for j, i in enumerate(idx):
                out[i] = logits[j, : len(items[i]["markers"])].tolist()
    return out  # type: ignore[return-value]


def length_bucketed_batches(n_items: int, lengths: Sequence[int], micro: int,
                            rng: random.Random) -> List[List[int]]:
    """Mescola, ordina per lunghezza a blocchi di 50 micro-batch (meno padding), rimescola."""
    order = list(range(n_items))
    rng.shuffle(order)
    batches = []
    block = micro * 50
    for s in range(0, len(order), block):
        chunk = sorted(order[s:s + block], key=lambda i: lengths[i])
        batches.extend(chunk[j:j + micro] for j in range(0, len(chunk), micro))
    rng.shuffle(batches)
    return batches


def train(model, train_items, val_items, args, device, pad_id) -> Dict[str, Any]:
    """RLCD come `train_ddp.py` del notebook, su una GPU: soft cross-entropy più policy
    gradient su G logit rumorosi (rumore annealed 0.4 → 0.1), ricompensa = regole di
    punteggio proprie di Laya (`proper_reward`, sferica 0.75, RPS 1.0)."""
    torch = _torch()
    from laya.common import proper_reward

    head_only = args.mode == "head"
    enc = [p for n, p in model.named_parameters() if n.startswith("encoder.")]
    head = [p for n, p in model.named_parameters() if not n.startswith("encoder.")]
    for p in enc:
        p.requires_grad_(not head_only)
    groups = [{"params": head, "lr": args.lr_head}]
    if not head_only:
        groups.append({"params": enc, "lr": args.lr_encoder})
        if args.grad_ckpt:
            model.encoder.gradient_checkpointing_enable(
                gradient_checkpointing_kwargs={"use_reentrant": False})
            model.head_checkpointing = True
    opt = torch.optim.AdamW(groups, weight_decay=0.01)
    steps = max(1, math.ceil(len(train_items) / (args.micro_batch * args.grad_accum)) * args.epochs)
    sched = torch.optim.lr_scheduler.CosineAnnealingLR(opt, T_max=steps, eta_min=1e-6)
    dt = amp_dtype(torch, device)
    scaler = torch.amp.GradScaler("cuda", enabled=dt == torch.float16)
    lengths = [len(it["ids"]) for it in train_items]
    rng = random.Random(args.seed)
    best = {"loss": float("inf"), "epoch": 0, "state": None}
    history = []
    t0 = time.time()
    done = 0
    total_batches = math.ceil(len(train_items) / args.micro_batch) * args.epochs
    for epoch in range(args.epochs):
        model.train()
        sigma = args.sigma_start + (args.sigma_end - args.sigma_start) * (
            epoch / max(1, args.epochs - 1))
        batches = length_bucketed_batches(len(train_items), lengths, args.micro_batch, rng)
        opt.zero_grad(set_to_none=True)
        run_loss = 0.0
        for b_idx, idx in enumerate(batches):
            b = collate([train_items[i] for i in idx], pad_id)
            with torch.autocast(device.type, dtype=dt, enabled=dt is not None):
                logits, act = model(b["input_ids"].to(device), b["attention_mask"].to(device),
                                    b["marker_pos"].to(device), b["marker_mask"].to(device),
                                    b["qtype"].to(device), detach_encoder=head_only)
            logits = logits.float()
            mask = b["marker_mask"].to(device)
            target = b["target"].to(device)
            loss_ce = -(target * torch.log_softmax(logits.masked_fill(~mask, -1e4), -1)).sum(-1).mean()
            loss = loss_ce
            if args.rl_weight > 0:
                k = mask.sum(-1, keepdim=True).float()
                eps = torch.randn((args.group_size,) + logits.shape, device=device) * sigma * mask
                eps = (eps - eps.sum(-1, keepdim=True) / k) * mask
                z = logits.detach().unsqueeze(0) + eps
                q = torch.softmax(z.masked_fill(~mask, -1e4), -1)
                with torch.no_grad():
                    r = proper_reward(q, target.unsqueeze(0), b["qtype"].to(device), mask,
                                      w_sph=0.75, w_rps=1.0)
                    adv = r - r.mean(0, keepdim=True)
                    adv = adv / (adv.std() + 1e-6)
                logp = -(((z - logits.unsqueeze(0)) ** 2) * mask).sum(-1) / (2 * sigma ** 2)
                loss = loss + args.rl_weight * (-(adv * logp).mean())
            loss = loss / args.grad_accum + 0.0 * act.sum()
            scaler.scale(loss).backward()
            if (b_idx + 1) % args.grad_accum == 0 or b_idx + 1 == len(batches):
                scaler.unscale_(opt)
                torch.nn.utils.clip_grad_norm_([p for p in model.parameters() if p.requires_grad], 1.0)
                scaler.step(opt)
                scaler.update()
                sched.step()
                opt.zero_grad(set_to_none=True)
            run_loss += float(loss_ce)
            done += 1
            if done == 20 or done % 200 == 0:
                el = time.time() - t0
                eta = el / done * (total_batches - done)
                print(f"  epoca {epoch + 1}/{args.epochs} · lotto {b_idx + 1}/{len(batches)} · "
                      f"CE {run_loss / (b_idx + 1):.4f} · {el / 60:.1f} min, ~{eta / 60:.0f} min alla fine",
                      flush=True)
        val_loss = float("nan")
        if val_items:
            vl = predict_logits(model, val_items, pad_id, device, args.eval_batch)
            val_loss = soft_nll([(lg, it["target"]) for lg, it in zip(vl, val_items)], 1.0)
            acc = sum(max(range(len(lg)), key=lambda i: lg[i]) == it["label"]
                      for lg, it in zip(vl, val_items)) / len(val_items)
            print(f"=== epoca {epoch + 1}: CE train {run_loss / max(1, len(batches)):.4f} · "
                  f"CE val {val_loss:.4f} · acc val {acc:.1%} · {(time.time() - t0) / 60:.1f} min",
                  flush=True)
            history.append({"epoch": epoch + 1, "train_ce": run_loss / max(1, len(batches)),
                            "val_ce": val_loss, "val_accuracy": acc})
            if args.keep_best and val_loss < best["loss"]:
                best = {"loss": val_loss, "epoch": epoch + 1,
                        "state": {k: v.detach().to("cpu", copy=True)
                                  for k, v in model.state_dict().items()}}
    if args.keep_best and best["state"] is not None and best["epoch"] != args.epochs:
        print(f"Tengo l'epoca {best['epoch']} (CE val più bassa)")
        model.load_state_dict(best["state"])
    return {"history": history, "best_epoch": best["epoch"] or args.epochs,
            "minutes": round((time.time() - t0) / 60, 1)}


def save_checkpoint(model, base_dir: str, out_dir: str, cfg: Dict[str, Any],
                    temps: Dict[str, Any], meta: Dict[str, Any]) -> None:
    """Salva nello stesso layout del repository HF: i pesi (F16, come upstream) e
    `rl_agent_config.json` con le temperature nuove; encoder/config.json e tokenizer/ si
    copiano dalla base (non cambiano), così `LayaModel::load` li legge come quelli base."""
    from safetensors.torch import save_file

    os.makedirs(os.path.join(out_dir, "encoder"), exist_ok=True)
    os.makedirs(os.path.join(out_dir, "tokenizer"), exist_ok=True)
    sd = {k: v.detach().half().contiguous().cpu() for k, v in model.state_dict().items()}
    save_file(sd, os.path.join(out_dir, "model.safetensors"), metadata={"format": "pt"})
    shutil.copyfile(os.path.join(base_dir, "encoder", "config.json"),
                    os.path.join(out_dir, "encoder", "config.json"))
    for name in os.listdir(os.path.join(base_dir, "tokenizer")):
        src = os.path.join(base_dir, "tokenizer", name)
        if os.path.isfile(src):
            shutil.copyfile(src, os.path.join(out_dir, "tokenizer", name))
    new_cfg = dict(cfg)
    base_t = list(cfg.get("temperature", [1.0, 1.0, 1.0]))
    new_cfg["temperature"] = [temps["choice"], base_t[1], base_t[2]]
    # Come il notebook: le temperature per bucket ereditate nasconderebbero quelle nuove.
    new_cfg["temperature_by_options"] = dict(temps["by_options"])
    new_cfg["fine_tuned"] = True
    new_cfg["model_name"] = "laya-traingame"
    with open(os.path.join(out_dir, "rl_agent_config.json"), "w", encoding="utf-8") as f:
        json.dump(new_cfg, f, indent=2, ensure_ascii=False)
    with open(os.path.join(out_dir, "traingame_finetune.json"), "w", encoding="utf-8") as f:
        json.dump(meta, f, indent=2, ensure_ascii=False)


def parity_check(out_dir: str, rows: Sequence[Row], logits, temps, device) -> Optional[float]:
    """Ricarica la checkpoint salvata con `laya.load` (il caricatore ufficiale) e confronta
    le probabilità su qualche riga: la differenza massima dovrebbe essere < 1e-2 (F16)."""
    try:
        import laya
        agent = laya.load(out_dir, device=str(device))
    except Exception as e:  # noqa: BLE001 - è solo un controllo
        print(f"(controllo con laya.load saltato: {e})")
        return None
    worst = 0.0
    for r, lg in list(zip(rows, logits))[:8]:
        q = {"type": "choice", "instructions": r.instructions,
             "criteria": list(r.crit) if all(v is None for v in r.crit.values()) else r.crit}
        ans = agent.predict(r.state, {QUESTION_ID: q})["answers"][QUESTION_ID]
        mine = softmax(lg, temperature_for({"temperature": [temps["choice"], 1, 1],
                                            "temperature_by_options": temps["by_options"]}, r.k))
        theirs = [ans["probabilities"][k] for k in r.keys]
        worst = max(worst, max(abs(a - b) for a, b in zip(mine, theirs)))
    return worst


# --- Main ------------------------------------------------------------------------------------

def parse_args(argv: Optional[Sequence[str]] = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0],
                                formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--data", help="cartella con train/val/test.jsonl (export_dataset)")
    p.add_argument("--out", default="laya-traingame-ft", help="cartella della checkpoint")
    p.add_argument("--base", default=BASE_REPO, help="repo HF o cartella della checkpoint base")
    p.add_argument("--revision", default=BASE_REVISION, help="commit della base su HF")
    p.add_argument("--check-data", action="store_true", help="valida il dataset e esce (stdlib)")
    p.add_argument("--check-checkpoint", metavar="DIR", help="controlla il layout e esce (stdlib)")
    p.add_argument("--eval-only", metavar="DIR", help="valuta questa checkpoint sul test ed esce")
    p.add_argument("--mode", choices=("full", "head"), default="full",
                   help="full = encoder + testa (come upstream); head = encoder congelato")
    p.add_argument("--targets", choices=("soft", "label"), default="soft",
                   help="soft = distribuzione dell'insegnante (RLCD); label = one-hot sull'etichetta")
    p.add_argument("--epochs", type=int, default=3)
    p.add_argument("--micro-batch", type=int, default=8)
    p.add_argument("--grad-accum", type=int, default=4)
    p.add_argument("--lr-encoder", type=float, default=2.5e-5)
    p.add_argument("--lr-head", type=float, default=1e-4)
    p.add_argument("--rl-weight", type=float, default=1.0, help="0 = solo soft cross-entropy")
    p.add_argument("--group-size", type=int, default=4)
    p.add_argument("--sigma-start", type=float, default=0.4)
    p.add_argument("--sigma-end", type=float, default=0.1)
    p.add_argument("--grad-ckpt", action="store_true", help="gradient checkpointing (meno memoria)")
    p.add_argument("--no-keep-best", dest="keep_best", action="store_false",
                   help="tieni l'ultima epoca invece della migliore in validazione")
    p.add_argument("--max-len", type=int, default=384,
                   help="token per riga (LayaOptions::max_len in Rust è 384)")
    p.add_argument("--max-train-rows", type=int, default=0, help="0 = tutte")
    p.add_argument("--eval-batch", type=int, default=32)
    p.add_argument("--no-baseline", dest="baseline", action="store_false",
                   help="salta la valutazione zero-shot della base sul test")
    p.add_argument("--seed", type=int, default=42)
    p.add_argument("--device", default=None, help="cuda, mps o cpu (default: il migliore)")
    p.add_argument("--no-zip", dest="zip", action="store_false", help="non creare OUT.zip")
    p.add_argument("--push-to-hub", metavar="REPO", help="carica la checkpoint (privata) su HF")
    return p.parse_args(argv)


def main(argv: Optional[Sequence[str]] = None) -> int:
    args = parse_args(argv)
    if args.check_checkpoint:
        base = args.base if os.path.isdir(args.base) else None
        problems = check_checkpoint(args.check_checkpoint, base)
        for pr in problems:
            print("✗", pr)
        print("checkpoint a posto" if not problems else f"{len(problems)} problemi")
        return 1 if problems else 0
    if not args.data:
        print("serve --data", file=sys.stderr)
        return 2
    data = load_dataset_dir(args.data)
    check_no_leakage(data)
    for split in SPLITS:
        print(f"[{split}] {json.dumps(dataset_stats(data[split]), ensure_ascii=False)}")
    if args.check_data:
        return 0

    torch = _torch()
    random.seed(args.seed)
    torch.manual_seed(args.seed)
    device = torch.device(args.device or ("cuda" if torch.cuda.is_available() else
                                          "mps" if torch.backends.mps.is_available() else "cpu"))
    if device.type == "cpu":
        print("ATTENZIONE: nessuna GPU, il training su CPU richiederebbe giorni.")
    if device.type == "cuda":
        print("GPU:", torch.cuda.get_device_name(0))

    # Con --eval-only la base non serve: si valuta direttamente quella cartella.
    base_dir = args.eval_only or resolve_base(args.base, args.revision)
    model_dir = base_dir
    tok, cfg, model = load_model(model_dir, device)
    head_max_len = int(cfg.get("head_max_len", 256))
    max_len = max(args.max_len, head_max_len + 16)
    pad_id = tok.pad_token_id
    report: Dict[str, Any] = {"base": args.base, "revision": args.revision,
                              "upstream_commit": UPSTREAM_COMMIT, "data": os.path.abspath(args.data)}

    test_items, test_rows, dropped = build_items(data["test"], tok, max_len, head_max_len)
    print(f"test: {len(test_items)} righe ({dropped} scartate: opzioni oltre head_max_len)")

    def eval_on_test(name: str, temps_cfg: Dict[str, Any]) -> Dict[str, Any]:
        if not test_items:
            return {}
        lg = predict_logits(model, test_items, pad_id, device, args.eval_batch)
        rep = evaluate(test_rows, lg, temps_cfg)
        print(format_report(name, rep))
        return rep

    if args.eval_only:
        report["eval"] = eval_on_test(f"== {args.eval_only} (test)", cfg)
        path = os.path.join(args.eval_only, "eval_report.json")
        with open(path, "w", encoding="utf-8") as f:
            json.dump(report, f, indent=2, ensure_ascii=False)
        print("Report:", path)
        return 0

    if args.baseline:
        report["baseline"] = eval_on_test("== base zero-shot (test, temperature della base)", cfg)

    train_rows = data["train"]
    calib_rows = data["val"]
    if len(calib_rows) < 50:
        train_rows, calib_rows = split_calibration(train_rows)
        print(f"val assente o piccolo: {len(calib_rows)} righe del train tenute da parte per "
              f"le temperature")
    if args.max_train_rows:
        train_rows = random.Random(args.seed).sample(train_rows,
                                                     min(args.max_train_rows, len(train_rows)))
    train_items, _, d1 = build_items(train_rows, tok, max_len, head_max_len, args.targets)
    calib_items, calib_rows, d2 = build_items(calib_rows, tok, max_len, head_max_len)
    lens = sorted(len(it["ids"]) for it in train_items) or [0]
    print(f"train: {len(train_items)} righe ({d1} scartate) · token per riga: mediana "
          f"{lens[len(lens) // 2]}, p95 {lens[int(len(lens) * 0.95)]}, max {lens[-1]} · "
          f"calibrazione: {len(calib_items)} ({d2} scartate)")
    report["training"] = train(model, train_items, calib_items, args, device, pad_id)

    calib_logits = predict_logits(model, calib_items, pad_id, device, args.eval_batch)
    temps = fit_temperatures(calib_rows, calib_logits)
    print(f"Temperature: choice {temps['choice']} · per opzioni {temps['by_options']} · "
          f"(per tipo di riga, solo informativo: {temps['by_kind']})")
    temps_cfg = {"temperature": [temps["choice"], 1.0, 1.0],
                 "temperature_by_options": temps["by_options"]}
    report["temperatures"] = temps
    report["calibration"] = evaluate(calib_rows, calib_logits, temps_cfg)["all"]
    test_logits = predict_logits(model, test_items, pad_id, device, args.eval_batch) \
        if test_items else []
    if test_items:
        report["finetuned"] = evaluate(test_rows, test_logits, temps_cfg)
        report["finetuned_uncalibrated"] = evaluate(test_rows, test_logits, cfg)["all"]
        print(format_report("== messa a punto (test, temperature fittate)", report["finetuned"]))

    report["args"] = {k: v for k, v in vars(args).items()}
    save_checkpoint(model, base_dir, args.out, cfg, temps, report)
    with open(os.path.join(args.out, "eval_report.json"), "w", encoding="utf-8") as f:
        json.dump(report, f, indent=2, ensure_ascii=False)
    problems = check_checkpoint(args.out, base_dir)
    if problems:
        print("ATTENZIONE, checkpoint:", problems)
    else:
        print(f"Checkpoint salvata in {args.out} (layout di LayaModel::load)")
    if test_items:
        diff = parity_check(args.out, test_rows, test_logits, temps, device)
        if diff is not None:
            print(f"Parità con laya.load: differenza massima {diff:.4f} sulle probabilità")
    if args.zip:
        archive = shutil.make_archive(args.out.rstrip("/"), "zip", args.out)
        print("Archivio da scaricare:", archive)
    if args.push_to_hub:
        from huggingface_hub import HfApi
        api = HfApi()
        api.create_repo(args.push_to_hub, private=True, exist_ok=True)
        api.upload_folder(folder_path=args.out, repo_id=args.push_to_hub)
        print("Caricata su https://huggingface.co/" + args.push_to_hub)
    return 0


if __name__ == "__main__":
    sys.exit(main())
