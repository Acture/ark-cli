#!/usr/bin/env python3
"""Export deterministic, lossless Unity TypeTree documents for Ark Solver.

This exporter preserves serialized metadata and raw TypeTree fields. It does not
infer operator mechanics or compile semantic behavior.
"""

from __future__ import annotations

import argparse
import dataclasses
import hashlib
import importlib.metadata
import json
import math
import re
import sys
from collections.abc import Mapping, Sequence
from pathlib import Path
from typing import Any, Protocol, TypedDict, cast


EXPORT_SCHEMA_VERSION: int = 1
EXPORTER_VERSION: str = "1"


class ScriptClass(TypedDict):
	class_name: str
	namespace: str
	assembly_name: str


class ExportJob(TypedDict):
	logical_path: str
	source_bundle: str
	source_path: str
	output_path: str
	source_sha256: str


class ToolReport(TypedDict):
	exporter_version: str
	python_version: str
	unitypy_version: str
	ark_unpacker_version: str


class TypeTreeNodeLike(Protocol):
	m_Index: int
	m_Level: int
	m_Type: str
	m_Name: str
	m_ByteSize: int
	m_Version: int
	m_TypeFlags: int
	m_MetaFlag: int
	m_RefTypeHash: int
	m_VariableCount: int | None
	m_Children: Sequence[TypeTreeNodeLike]


@dataclasses.dataclass(frozen=True, slots=True)
class Arguments:
	jobs: Path
	ark_unpacker_root: Path
	report: Path


def _parse_arguments(argv: Sequence[str]) -> Arguments:
	parser = argparse.ArgumentParser(description=__doc__)
	parser.add_argument("--jobs", required=True, type=Path)
	parser.add_argument("--ark-unpacker-root", required=True, type=Path)
	parser.add_argument("--report", required=True, type=Path)
	parsed = parser.parse_args(argv)
	return Arguments(
		jobs=cast(Path, parsed.jobs),
		ark_unpacker_root=cast(Path, parsed.ark_unpacker_root),
		report=cast(Path, parsed.report),
	)


def _load_jobs(path: Path) -> list[ExportJob]:
	raw: object = json.loads(path.read_text(encoding="utf-8"))
	if not isinstance(raw, list) or not raw:
		raise ValueError("jobs document must be a non-empty JSON array")
	jobs: list[ExportJob] = []
	for index, value in enumerate(raw):
		if not isinstance(value, dict):
			raise ValueError(f"jobs[{index}] must be an object")
		record: dict[str, object] = cast(dict[str, object], value)
		required: tuple[str, ...] = (
			"logical_path",
			"source_bundle",
			"source_path",
			"output_path",
			"source_sha256",
		)
		if set(record) != set(required):
			raise ValueError(f"jobs[{index}] must contain exactly {required!r}")
		if not all(
			isinstance(record[field], str) and record[field] for field in required
		):
			raise ValueError(f"jobs[{index}] fields must be non-empty strings")
		sha256: str = cast(str, record["source_sha256"])
		if re.fullmatch(r"[0-9a-f]{64}", sha256) is None:
			raise ValueError(f"jobs[{index}].source_sha256 must be lowercase SHA-256")
		jobs.append(
			{
				"logical_path": cast(str, record["logical_path"]),
				"source_bundle": cast(str, record["source_bundle"]),
				"source_path": cast(str, record["source_path"]),
				"output_path": cast(str, record["output_path"]),
				"source_sha256": sha256,
			}
		)
	return sorted(jobs, key=lambda job: job["logical_path"].encode("utf-8"))


def _configure_unitypy(ark_unpacker_root: Path) -> tuple[Any, str]:
	root: Path = ark_unpacker_root.resolve(strict=True)
	sys.path.insert(0, str(root))
	from UnityPy.enums.BundleFile import CompressionFlags
	from UnityPy.helpers import CompressionHelper

	from src.lz4ak.Block import decompress_lz4ak

	import UnityPy

	CompressionHelper.DECOMPRESSION_MAP[CompressionFlags.LZHAM] = decompress_lz4ak
	main_source: str = (root / "Main.py").read_text(encoding="utf-8")
	match: re.Match[str] | None = re.search(
		r'^ARKUNPACKER_VERSION\s*=\s*["\']([^"\']+)["\']',
		main_source,
		flags=re.MULTILINE,
	)
	ark_unpacker_version: str = match.group(1) if match else "unknown"
	return UnityPy, ark_unpacker_version


def _hex_or_none(value: object) -> str | None:
	if value is None:
		return None
	if not isinstance(value, (bytes, bytearray, memoryview)):
		raise TypeError(f"expected bytes metadata, found {type(value).__name__}")
	return bytes(value).hex()


def _json_value(value: object) -> object:
	"""Convert a TypeTree value to reversible, strict JSON values."""
	if value is None or isinstance(value, (bool, int)):
		return value
	if isinstance(value, str):
		try:
			value.encode("utf-8")
		except UnicodeEncodeError:
			# Unity stores some TextAsset scripts as raw bytes, which UnityPy surfaces as a str
			# holding unpaired UTF-16 surrogates. Such a value is not representable in JSON at
			# all, so preserve its exact code units rather than failing the export or mangling
			# them. `surrogatepass` round-trips them byte for byte.
			return {"$wtf8": value.encode("utf-8", "surrogatepass").hex()}
		return value
	if isinstance(value, float):
		if math.isfinite(value):
			return value
		return {"$float": repr(value)}
	if isinstance(value, (bytes, bytearray, memoryview)):
		return {"$bytes": bytes(value).hex()}
	if isinstance(value, Mapping):
		return {
			str(key): _json_value(item)
			for key, item in sorted(
				value.items(), key=lambda pair: str(pair[0]).encode("utf-8")
			)
		}
	if isinstance(value, Sequence):
		return [_json_value(item) for item in value]
	if dataclasses.is_dataclass(value) and not isinstance(value, type):
		return _json_value(dataclasses.asdict(value))
	if hasattr(value, "__dict__"):
		return _json_value(vars(value))
	raise TypeError(f"unsupported TypeTree value {type(value).__name__}")


def _flatten_schema(root: TypeTreeNodeLike | None) -> list[dict[str, object]]:
	if root is None:
		return []
	result: list[dict[str, object]] = []

	def visit(node: TypeTreeNodeLike, parent_index: int | None) -> None:
		output_index: int = len(result)
		result.append(
			{
				"index": int(node.m_Index),
				"level": int(node.m_Level),
				"type": node.m_Type,
				"name": node.m_Name,
				"byte_size": int(node.m_ByteSize),
				"version": int(node.m_Version),
				"type_flags": int(node.m_TypeFlags),
				"meta_flags": int(node.m_MetaFlag),
				"ref_type_hash": int(node.m_RefTypeHash),
				"variable_count": (
					None if node.m_VariableCount is None else int(node.m_VariableCount)
				),
				"parent_index": parent_index,
			}
		)
		for child in node.m_Children:
			visit(child, output_index)

	visit(root, None)
	return result


def _direct_script_class(serialized_type: Any) -> ScriptClass | None:
	class_name: object = getattr(serialized_type, "m_ClassName", None)
	namespace: object = getattr(serialized_type, "m_NameSpace", None)
	assembly_name: object = getattr(serialized_type, "m_AssemblyName", None)
	if not isinstance(class_name, str) or not class_name:
		return None
	return {
		"class_name": class_name,
		"namespace": namespace if isinstance(namespace, str) else "",
		"assembly_name": assembly_name if isinstance(assembly_name, str) else "",
	}


def _resolved_script_class(reader: Any) -> ScriptClass | None:
	if int(reader.class_id) != 114:
		return None
	try:
		component: Any = reader.read()
		script_pointer: Any = getattr(component, "m_Script", None)
		if script_pointer is None:
			return None
		script: Any = script_pointer.read()
		class_name: object = getattr(script, "m_ClassName", None)
		if not isinstance(class_name, str) or not class_name:
			return None
		return {
			"class_name": class_name,
			"namespace": str(getattr(script, "m_Namespace", "")),
			"assembly_name": str(getattr(script, "m_AssemblyName", "")),
		}
	except Exception:
		return None


def _serialized_type_document(
	type_index: int,
	serialized_type: Any,
	script_class: ScriptClass | None,
) -> dict[str, object]:
	node: TypeTreeNodeLike | None = getattr(serialized_type, "node", None)
	return {
		"type_index": type_index,
		"class_id": int(serialized_type.class_id),
		"is_stripped_type": bool(serialized_type.is_stripped_type),
		"script_type_index": int(serialized_type.script_type_index),
		"script_id": _hex_or_none(serialized_type.script_id),
		"old_type_hash": _hex_or_none(serialized_type.old_type_hash),
		"type_dependencies": [int(item) for item in serialized_type.type_dependencies],
		"script_class": script_class,
		"schema": _flatten_schema(node),
	}


def _external_document(file_id: int, external: Any) -> dict[str, object]:
	return {
		"file_id": file_id,
		"guid": _hex_or_none(external.guid),
		"type": int(external.type),
		"path": str(external.path),
		"name": str(external.name),
	}


def _serialized_file_document(serialized_file: Any) -> dict[str, object]:
	readers: list[Any] = sorted(
		serialized_file.objects.values(), key=lambda reader: int(reader.path_id)
	)
	resolved_by_type: dict[int, ScriptClass | None] = {}
	for reader in readers:
		type_index: int = int(reader.type_id)
		if type_index not in resolved_by_type:
			resolved_by_type[type_index] = _direct_script_class(
				reader.serialized_type
			) or _resolved_script_class(reader)

	types: list[dict[str, object]] = []
	for type_index, serialized_type in enumerate(serialized_file.types):
		script_class: ScriptClass | None = _direct_script_class(serialized_type)
		if script_class is None:
			script_class = resolved_by_type.get(type_index)
		types.append(
			_serialized_type_document(type_index, serialized_type, script_class)
		)

	objects: list[dict[str, object]] = []
	for reader in readers:
		try:
			fields: object = _json_value(reader.read_typetree())
		except Exception as error:
			raise RuntimeError(
				f"cannot read complete TypeTree for {serialized_file.name} "
				f"path_id={reader.path_id} class_id={reader.class_id}: {error}"
			) from error
		objects.append(
			{
				"path_id": int(reader.path_id),
				"class_id": int(reader.class_id),
				"type_index": int(reader.type_id),
				"unity_type": str(reader.type.name),
				"byte_start": int(reader.byte_start),
				"byte_size": int(reader.byte_size),
				"script_class": resolved_by_type[int(reader.type_id)],
				"fields": fields,
			}
		)

	return {
		"cab_name": str(serialized_file.name),
		"unity_version": str(serialized_file.unity_version),
		"target_platform": int(serialized_file.target_platform),
		"enable_type_tree": bool(serialized_file._enable_type_tree),
		"externals": [
			_external_document(file_id, external)
			for file_id, external in enumerate(serialized_file.externals, start=1)
		],
		"types": types,
		"objects": objects,
	}


def _serialized_files(bundle: Any) -> list[Any]:
	result: list[Any] = []

	def visit(file: Any) -> None:
		if file.__class__.__name__ == "SerializedFile":
			result.append(file)
			return
		for child in getattr(file, "files", {}).values():
			visit(child)

	visit(bundle)
	return sorted(result, key=lambda file: str(file.name).encode("utf-8"))


def _bundle_document(job: ExportJob, bundle: Any) -> dict[str, object]:
	return {
		"schema_version": EXPORT_SCHEMA_VERSION,
		"source_bundle": job["source_bundle"],
		"source_sha256": job["source_sha256"],
		"bundle": {
			"signature": str(bundle.signature),
			"format_version": int(bundle.version),
			"player_version": str(bundle.version_player),
			"engine_version": str(bundle.version_engine),
		},
		"serialized_files": [
			_serialized_file_document(file) for file in _serialized_files(bundle)
		],
	}


def _write_json(path: Path, value: object) -> None:
	path.parent.mkdir(parents=True, exist_ok=True)
	with path.open("w", encoding="utf-8", newline="\n") as output:
		json.dump(
			value,
			output,
			ensure_ascii=False,
			allow_nan=False,
			indent=2,
			sort_keys=True,
			separators=(",", ": "),
		)
		output.write("\n")


def _read_verified_source(job: ExportJob) -> bytes:
	"""Read, hash, and freeze one bundle before handing it to UnityPy."""
	source_bytes: bytes = Path(job["source_path"]).read_bytes()
	actual_sha256: str = hashlib.sha256(source_bytes).hexdigest()
	expected_sha256: str = job["source_sha256"]
	if actual_sha256 != expected_sha256:
		raise ValueError(
			f"source bundle hash mismatch for {job['logical_path']}: "
			f"expected {expected_sha256}, found {actual_sha256}"
		)
	return source_bytes


def _export_jobs(jobs: Sequence[ExportJob], unitypy: Any) -> None:
	for index, job in enumerate(jobs, start=1):
		print(
			f"[{index}/{len(jobs)}] exporting {job['logical_path']}",
			file=sys.stderr,
			flush=True,
		)
		source_path: Path = Path(job["source_path"])
		source_bytes: bytes = _read_verified_source(job)
		environment: Any = unitypy.load(
			source_bytes,
			path=str(source_path.parent),
		)
		loaded_files: list[Any] = list(environment.files.values())
		if len(loaded_files) != 1:
			raise ValueError(
				f"{job['source_path']} loaded {len(loaded_files)} top-level files; "
				"expected exactly one Unity AssetBundle"
			)
		bundle: Any = loaded_files[0]
		if bundle.__class__.__name__ != "BundleFile":
			raise TypeError(f"{job['source_path']} is not a Unity AssetBundle")
		document: dict[str, object] = _bundle_document(job, bundle)
		_write_json(Path(job["output_path"]), document)


def _tool_report(ark_unpacker_version: str) -> ToolReport:
	return {
		"exporter_version": EXPORTER_VERSION,
		"python_version": ".".join(str(part) for part in sys.version_info[:3]),
		"unitypy_version": importlib.metadata.version("UnityPy"),
		"ark_unpacker_version": ark_unpacker_version,
	}


def main(argv: Sequence[str] | None = None) -> int:
	arguments: Arguments = _parse_arguments(sys.argv[1:] if argv is None else argv)
	jobs: list[ExportJob] = _load_jobs(arguments.jobs)
	unitypy, ark_unpacker_version = _configure_unitypy(arguments.ark_unpacker_root)
	_export_jobs(jobs, unitypy)
	_write_json(arguments.report, _tool_report(ark_unpacker_version))
	return 0


if __name__ == "__main__":
	raise SystemExit(main())
