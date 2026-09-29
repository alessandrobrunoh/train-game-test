#!/usr/bin/env python3
"""Test delle parti di finetune.py che non usano torch (solo libreria standard).

    python3 tools/laya-finetune/test_finetune_data.py
    LAYA_DATASET_DIR=laya-dataset python3 tools/laya-finetune/test_finetune_data.py  # + un export vero
"""

import json
import math
import os
import random
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import finetune as ft  # noqa: E402

SAMPLE = os.path.join(HERE, "sample")
TINY = os.path.join(HERE, "..", "..", "crates", "sim-laya", "src", "laya", "testdata", "tiny")


def typed_decisions_case():
    """Un caso nello stile di LocalLLaMA/typed-decisions: colonne come stringhe JSON,
    criteri con chiavi opache."""
    return {
        "id": "td-1",
        "state": json.dumps({"ticket": "rimborso negato"}),
        "questions": json.dumps({"route": {"type": "choice", "instructions": "Chi risponde?",
                                           "criteria": {"A": "fatturazione", "B": "supporto"}}}),
        "gold": json.dumps({"route": {"label": "B",
                                      "probabilities": {"A": 0.2, "B": 0.6}}}),
    }


class DatasetTest(unittest.TestCase):
    def test_sample_loads_and_matches_the_schema(self):
        data = ft.load_dataset_dir(SAMPLE)
        ft.check_no_leakage(data)
        for split in ft.SPLITS:
            self.assertTrue(data[split], split)
            for r in data[split]:
                self.assertEqual(r.split, split)
                self.assertIn(r.kind, ("action", "deliberation"))
                self.assertIn(r.source, ("sim", "obvious"))
                self.assertTrue(2 <= r.k <= 5)
                self.assertAlmostEqual(sum(r.target), 1.0, places=6)
                self.assertTrue(0 <= r.label < r.k)
                self.assertGreater(r.target[r.label], 0.0)
                # Lista di descrizioni → opzioni senza lettere (OptionLabels::Plain in Rust).
                self.assertTrue(all(v is None for v in r.crit.values()))
                self.assertEqual(list(r.crit), r.keys)
                self.assertEqual(len(r.tags), r.k)
                self.assertIsInstance(r.state, str)
        stats = ft.dataset_stats(data["train"])
        self.assertEqual(stats["rows"], len(data["train"]))
        self.assertIn("action/sim", stats["by_kind_source"])

    def test_sample_rows_are_valid_export_lines(self):
        with open(os.path.join(SAMPLE, "train.jsonl"), encoding="utf-8") as f:
            raw = json.loads(f.readline())
        self.assertEqual(raw["schema"], 1)
        q = raw["questions"][ft.QUESTION_ID]
        g = raw["gold"][ft.QUESTION_ID]
        self.assertEqual(q["type"], "choice")
        self.assertEqual(q["criteria"][g["label_index"]], g["label"])
        self.assertEqual(set(g["probabilities"]), set(q["criteria"]))

    def test_typed_decisions_rows_are_accepted(self):
        r = ft.parse_row(typed_decisions_case(), "test")
        self.assertEqual(r.keys, ["A", "B"])
        self.assertEqual(r.label, 1)
        self.assertEqual(r.crit, {"A": "fatturazione", "B": "supporto"})
        self.assertAlmostEqual(r.target[1], 0.75)
        self.assertEqual(r.state, {"ticket": "rimborso negato"})

    def test_bad_rows_are_rejected(self):
        with open(os.path.join(SAMPLE, "train.jsonl"), encoding="utf-8") as f:
            good = json.loads(f.readline())
        cases = []
        dup = json.loads(json.dumps(good))
        crit = dup["questions"][ft.QUESTION_ID]["criteria"]
        crit[1] = crit[0]
        cases.append(dup)
        wrong_label = json.loads(json.dumps(good))
        g = wrong_label["gold"][ft.QUESTION_ID]
        g["label_index"] = (g["label_index"] + 1) % len(crit)
        cases.append(wrong_label)
        no_gold = json.loads(json.dumps(good))
        no_gold["gold"] = {}
        cases.append(no_gold)
        empty_state = json.loads(json.dumps(good))
        empty_state["state"] = ""
        cases.append(empty_state)
        score = json.loads(json.dumps(good))
        score["questions"][ft.QUESTION_ID]["type"] = "score"
        cases.append(score)
        for c in cases:
            with self.assertRaises(ValueError):
                ft.parse_row(c)

    def test_leakage_is_detected(self):
        data = ft.load_dataset_dir(SAMPLE)
        data["test"] = data["test"] + [data["train"][0]]
        with self.assertRaises(ValueError):
            ft.check_no_leakage(data)

    def test_calibration_slice_is_disjoint(self):
        rows = ft.load_dataset_dir(SAMPLE)["train"] * 50
        for i, r in enumerate(rows):
            rows[i] = ft.Row(**{**r.__dict__, "id": f"{r.id}-{i}"})
        train, calib = ft.split_calibration(rows)
        self.assertEqual(len(calib), min(ft.CALIB_MAX, len(rows) // 10))
        self.assertEqual(len(train) + len(calib), len(rows))
        self.assertFalse({r.id for r in train} & {r.id for r in calib})
        again = ft.split_calibration(rows)
        self.assertEqual([r.id for r in again[1]], [r.id for r in calib])

    def test_check_data_cli(self):
        self.assertEqual(ft.main(["--data", SAMPLE, "--check-data"]), 0)

    @unittest.skipUnless(os.environ.get("LAYA_DATASET_DIR"), "LAYA_DATASET_DIR non impostata")
    def test_real_export(self):
        data = ft.load_dataset_dir(os.environ["LAYA_DATASET_DIR"])
        ft.check_no_leakage(data)
        self.assertTrue(data["train"])


class CalibrationTest(unittest.TestCase):
    def test_buckets_match_rust_and_upstream(self):
        self.assertEqual(ft.temperature_bucket(1), "choice:2")
        self.assertEqual(ft.temperature_bucket(2), "choice:2")
        self.assertEqual(ft.temperature_bucket(5), "choice:3-5")
        self.assertEqual(ft.temperature_bucket(6), "choice:6-10")
        self.assertEqual(ft.temperature_bucket(11), "choice:11+")

    def test_temperature_lookup_matches_agent_config(self):
        # Lo stesso caso di `temperature_lookup_prefers_buckets` in config.rs.
        cfg = {"temperature": [1.6, 1.2, 1.9],
               "temperature_by_options": {"choice:3-5": 1.75, "choice:11+": 0.1}}
        self.assertEqual(ft.temperature_for(cfg, 3), 1.75)
        self.assertEqual(ft.temperature_for(cfg, 2), 1.6)
        self.assertEqual(ft.temperature_for(cfg, 12), 0.5)

    def test_fit_recovers_a_known_temperature(self):
        rng = random.Random(1)
        pairs = []
        for _ in range(300):
            k = rng.choice([2, 3, 5])
            logits = [rng.gauss(0, 3) for _ in range(k)]
            pairs.append((logits, ft.softmax(logits, 2.0)))
        self.assertAlmostEqual(ft.fit_temperature(pairs), 2.0, delta=0.02)
        # Il minimo è dentro i limiti di Laya.
        sharp = [(lg, ft.softmax(lg, 0.2)) for lg, _ in pairs]
        self.assertAlmostEqual(ft.fit_temperature(sharp), ft.TEMP_MIN, delta=1e-3)
        self.assertEqual(ft.fit_temperature(pairs[:5]), 1.0)

    def test_fit_temperatures_per_bucket(self):
        data = ft.load_dataset_dir(SAMPLE)
        rows = (data["train"] + data["val"] + data["test"]) * 4
        rng = random.Random(2)
        logits = [[rng.gauss(0, 1) + 3 * t for t in r.target] for r in rows]
        temps = ft.fit_temperatures(rows, logits, min_items=10)
        self.assertTrue(ft.TEMP_MIN <= temps["choice"] <= ft.TEMP_MAX)
        self.assertIn("choice:3-5", temps["by_options"])
        self.assertEqual(temps["count"], len(rows))
        for t in list(temps["by_options"].values()) + list(temps["by_kind"].values()):
            self.assertTrue(ft.TEMP_MIN <= t <= ft.TEMP_MAX)

    def test_ece(self):
        self.assertAlmostEqual(ft.ece_score([1.0, 1.0], [1.0, 1.0]), 0.0)
        self.assertAlmostEqual(ft.ece_score([0.9, 0.9], [0.0, 0.0]), 0.9)
        self.assertAlmostEqual(ft.ece_score([0.5, 0.5, 0.5, 0.5], [1, 0, 1, 0]), 0.0)
        self.assertTrue(math.isnan(ft.ece_score([], [])))

    def test_evaluate_reports_groups(self):
        data = ft.load_dataset_dir(SAMPLE)
        rows = data["test"]
        # Logit che copiano l'insegnante: accuratezza piena sulle etichette avide.
        logits = [[math.log(max(p, 1e-6)) for p in r.target] for r in rows]
        rep = ft.evaluate(rows, logits, {"temperature": [1.0, 1.0, 1.0]})
        self.assertEqual(rep["all"]["n"], len(rows))
        self.assertIn("action", rep["kind"])
        self.assertTrue(0.0 <= rep["all"]["ece"] <= 1.0)
        greedy = [r for r in rows if r.target[r.label] == max(r.target)]
        self.assertGreaterEqual(rep["all"]["accuracy"], len(greedy) / len(rows) - 0.3)
        self.assertIn("tutto", ft.format_report("x", rep))


class CheckpointTest(unittest.TestCase):
    def test_tiny_fixture_has_the_layout_rust_loads(self):
        self.assertEqual(ft.check_checkpoint(TINY), [])
        self.assertEqual(ft.check_checkpoint(TINY, base=TINY), [])
        header = ft.safetensors_header(os.path.join(TINY, "model.safetensors"))
        self.assertIn("type_emb.weight", header)

    def test_problems_are_reported(self):
        with tempfile.TemporaryDirectory() as tmp:
            ckpt = os.path.join(tmp, "ckpt")
            shutil.copytree(TINY, ckpt)
            with open(os.path.join(ckpt, "rl_agent_config.json"), encoding="utf-8") as f:
                cfg = json.load(f)
            cfg["temperature_by_options"] = {"choice:2": 0.1}
            with open(os.path.join(ckpt, "rl_agent_config.json"), "w", encoding="utf-8") as f:
                json.dump(cfg, f)
            problems = ft.check_checkpoint(ckpt)
            self.assertTrue(any("choice:2" in p for p in problems), problems)
            os.remove(os.path.join(ckpt, "tokenizer", "tokenizer_config.json"))
            self.assertEqual(ft.check_checkpoint(ckpt), ["manca tokenizer/tokenizer_config.json"])
            self.assertEqual(ft.main(["--check-checkpoint", ckpt]), 1)


if __name__ == "__main__":
    unittest.main(verbosity=2)
