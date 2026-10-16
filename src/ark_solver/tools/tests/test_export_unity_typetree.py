from __future__ import annotations

import hashlib
import importlib.util
import json
import sys
import tempfile
import unittest
from dataclasses import dataclass, field
from pathlib import Path


SCRIPTS_DIR: Path = Path(__file__).resolve().parents[1]
MODULE_PATH: Path = SCRIPTS_DIR / "export_unity_typetree.py"
MODULE_SPEC = importlib.util.spec_from_file_location(
	"export_unity_typetree", MODULE_PATH
)
if MODULE_SPEC is None or MODULE_SPEC.loader is None:
	raise RuntimeError(f"could not load exporter module from {MODULE_PATH}")
exporter = importlib.util.module_from_spec(MODULE_SPEC)
sys.modules[MODULE_SPEC.name] = exporter
MODULE_SPEC.loader.exec_module(exporter)


@dataclass(slots=True)
class FakeNode:
	m_Index: int
	m_Level: int
	m_Type: str
	m_Name: str
	m_ByteSize: int = -1
	m_Version: int = 1
	m_TypeFlags: int = 0
	m_MetaFlag: int = 0
	m_RefTypeHash: int = 0
	m_VariableCount: int | None = None
	m_Children: list[FakeNode] = field(default_factory=list)


@dataclass(slots=True)
class FakeBundle:
	signature: str = "UnityFS"
	version: int = 8
	version_player: str = "5.x.x"
	version_engine: str = "2021.3.39f1"
	files: dict[str, object] = field(default_factory=dict)


class BundleFile(FakeBundle):
	pass


@dataclass(slots=True)
class FakeEnvironment:
	files: dict[str, object]


@dataclass(slots=True)
class ReplacingUnityPy:
	source_path: Path
	replacement: bytes
	loaded_bytes: bytes | None = None
	load_path: str | None = None

	def load(self, source: bytes, *, path: str) -> FakeEnvironment:
		self.source_path.write_bytes(self.replacement)
		self.loaded_bytes = source
		self.load_path = path
		bundle = BundleFile(version_player=source.decode("utf-8"))
		return FakeEnvironment(files={"snapshot": bundle})


@dataclass(slots=True)
class RejectUnexpectedLoad:
	called: bool = False

	def load(self, source: bytes, *, path: str) -> FakeEnvironment:
		self.called = True
		raise AssertionError(
			f"unexpected UnityPy load from {path}: {len(source)} bytes"
		)


class ExporterTests(unittest.TestCase):
	def test_source_snapshot_hashes_the_exact_bytes_returned_for_parsing(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			path = Path(directory) / "source.ab"
			path.write_bytes(b"verified bundle bytes")
			job: exporter.ExportJob = {
				"logical_path": "source",
				"source_bundle": "assets/source.ab",
				"source_path": str(path),
				"output_path": str(Path(directory) / "export.json"),
				"source_sha256": hashlib.sha256(b"verified bundle bytes").hexdigest(),
			}

			self.assertEqual(
				exporter._read_verified_source(job),
				b"verified bundle bytes",
			)

	def test_export_parses_verified_snapshot_when_source_path_is_replaced(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			root = Path(directory)
			source_path = root / "source.ab"
			output_path = root / "export.json"
			original = b"original bundle snapshot"
			replacement = b"replacement bundle"
			source_path.write_bytes(original)
			job: exporter.ExportJob = {
				"logical_path": "source",
				"source_bundle": "assets/source.ab",
				"source_path": str(source_path),
				"output_path": str(output_path),
				"source_sha256": hashlib.sha256(original).hexdigest(),
			}
			unitypy = ReplacingUnityPy(source_path, replacement)

			exporter._export_jobs([job], unitypy)

			document = json.loads(output_path.read_text(encoding="utf-8"))
			self.assertEqual(unitypy.loaded_bytes, original)
			self.assertEqual(unitypy.load_path, str(root))
			self.assertEqual(document["bundle"]["player_version"], original.decode())
			self.assertEqual(source_path.read_bytes(), replacement)

	def test_source_hash_mismatch_rejects_before_unitypy_parsing(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			root = Path(directory)
			source_path = root / "source.ab"
			source_path.write_bytes(b"unexpected")
			job: exporter.ExportJob = {
				"logical_path": "source",
				"source_bundle": "assets/source.ab",
				"source_path": str(source_path),
				"output_path": str(root / "export.json"),
				"source_sha256": hashlib.sha256(b"expected").hexdigest(),
			}
			unitypy = RejectUnexpectedLoad()

			with self.assertRaisesRegex(ValueError, "source bundle hash mismatch"):
				exporter._export_jobs([job], unitypy)

			self.assertFalse(unitypy.called)

	def test_document_source_bundle_uses_apk_path_not_logical_key(self) -> None:
		job: exporter.ExportJob = {
			"logical_path": "[uc]skills",
			"source_bundle": "assets/AB/Android/battle/prefabs/[uc]skills.ab",
			"source_path": "/tmp/source.ab",
			"output_path": "/tmp/export.json",
			"source_sha256": "0" * 64,
		}

		document = exporter._bundle_document(job, FakeBundle())

		self.assertEqual(document["source_bundle"], job["source_bundle"])

	def test_schema_is_flattened_preorder_with_output_parent_indices(self) -> None:
		root = FakeNode(
			0,
			0,
			"MonoBehaviour",
			"Base",
			m_Children=[
				FakeNode(7, 1, "int", "count"),
				FakeNode(
					9,
					1,
					"vector",
					"items",
					m_Children=[FakeNode(12, 2, "Array", "Array")],
				),
			],
		)

		schema = exporter._flatten_schema(root)

		self.assertEqual([node["index"] for node in schema], [0, 7, 9, 12])
		self.assertEqual([node["parent_index"] for node in schema], [None, 0, 0, 2])

	def test_raw_values_preserve_bytes_and_non_finite_floats(self) -> None:
		value = exporter._json_value(
			{"payload": b"\x00\xff", "nan": float("nan"), "refs": (3, 5)}
		)

		self.assertEqual(
			value,
			{
				"nan": {"$float": "nan"},
				"payload": {"$bytes": "00ff"},
				"refs": [3, 5],
			},
		)

	def test_jobs_sort_bytewise_and_require_exact_lowercase_sha256(self) -> None:
		with tempfile.TemporaryDirectory() as directory:
			path = Path(directory) / "jobs.json"
			path.write_text(
				json.dumps(
					[
						{
							"logical_path": "z",
							"source_bundle": "assets/z.ab",
							"source_path": "/source/z.ab",
							"output_path": "/output/z.json",
							"source_sha256": "f" * 64,
						},
						{
							"logical_path": "a",
							"source_bundle": "assets/a.ab",
							"source_path": "/source/a.ab",
							"output_path": "/output/a.json",
							"source_sha256": "0" * 64,
						},
					]
				),
				encoding="utf-8",
			)

			jobs = exporter._load_jobs(path)

			self.assertEqual([job["logical_path"] for job in jobs], ["a", "z"])

			jobs[0]["source_sha256"] = "A" * 64
			path.write_text(json.dumps(jobs), encoding="utf-8")
			with self.assertRaisesRegex(ValueError, "lowercase SHA-256"):
				exporter._load_jobs(path)


if __name__ == "__main__":
	unittest.main()
