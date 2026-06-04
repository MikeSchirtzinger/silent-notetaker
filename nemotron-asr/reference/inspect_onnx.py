#!/usr/bin/env python3
"""Dump ground-truth I/O + op coverage for the Nemotron streaming ONNX files.

This is the de-risk step: confirm the real tensor names/shapes/dtypes (resolving
the `encoded` vs `outputs` ambiguity from the research) and check that every op
used is something onnxruntime-web's WASM kernels support (i.e. that the INT8
quant didn't introduce exotic ops). onnxruntime Python == onnxruntime-web kernels.
"""
import sys, collections
import onnx
from onnx import TensorProto

DT = {v: k for k, v in TensorProto.DataType.items()}

def shape_of(t):
    tp = t.type.tensor_type
    dims = []
    for d in tp.shape.dim:
        if d.HasField("dim_value"):
            dims.append(str(d.dim_value))
        elif d.HasField("dim_param"):
            dims.append(d.dim_param)
        else:
            dims.append("?")
    return f"{DT.get(tp.elem_type,'?')}[{', '.join(dims)}]"

def dump(path):
    print("=" * 78)
    print(f"FILE: {path}")
    print("=" * 78)
    m = onnx.load(path)
    g = m.graph
    print("opsets:", [(op.domain or "ai.onnx", op.version) for op in m.opset_import])
    print(f"\nINPUTS ({len(g.input)}):")
    for t in g.input:
        print(f"  {t.name:32s} {shape_of(t)}")
    print(f"\nOUTPUTS ({len(g.output)}):")
    for t in g.output:
        print(f"  {t.name:32s} {shape_of(t)}")
    ops = collections.Counter(n.op_type for n in g.node)
    domains = collections.Counter((n.domain or "ai.onnx") for n in g.node)
    print(f"\nNODES: {len(g.node)} | domains: {dict(domains)}")
    print("OP HISTOGRAM:")
    for op, c in ops.most_common():
        print(f"  {c:4d}  {op}")
    # flag anything that smells like a coverage risk on WASM/WebGPU
    risky = {"LSTM", "GRU", "RNN", "ScatterND", "ScatterElements", "GatherND",
             "NonZero", "If", "Loop", "Scan", "SequenceAt", "SequenceInsert"}
    quant = {"DynamicQuantizeLinear", "QuantizeLinear", "DequantizeLinear",
             "MatMulInteger", "ConvInteger", "QLinearMatMul", "QLinearConv",
             "MatMulNBits", "DynamicQuantizeMatMul", "MatMulIntegerToFloat"}
    present_risky = sorted(set(ops) & risky)
    present_quant = sorted(set(ops) & quant)
    custom = sorted({(n.domain) for n in g.node if n.domain and n.domain != "ai.onnx"})
    if present_quant: print("\n[INT8 quant ops present]:", present_quant)
    if present_risky: print("[control-flow / recurrent / scatter ops]:", present_risky)
    if custom: print("[non-standard op domains]:", custom)
    print()

if __name__ == "__main__":
    for p in sys.argv[1:] or ["models/encoder.onnx", "models/decoder_joint.onnx"]:
        dump(p)
