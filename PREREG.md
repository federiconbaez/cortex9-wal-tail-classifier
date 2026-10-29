# Pre-registro / Pre-registration

**ES.** Este archivo y `prereg.toml` se commitean y se publican **antes** de medir. Fijan el modelo de fallas, el workload, las semillas y los umbrales. Cualquier cambio posterior es un desvío del protocolo y se reporta en el README.

**EN.** This file and `prereg.toml` are committed and published **before** any measurement. They fix the fault model, workload, seeds and thresholds. Any later change is a protocol deviation and is reported in the README.

## Hypothesis / Hipótesis

A single-node WAL can tell, at recovery, a torn tail (a batch whose fsync never returned) from corruption of acknowledged records, without replicas, using redundant evidence in every 512-byte sector: a CRC over the sector, the log epoch, the batch sequence number and the sector's own address. The baseline (one CRC per record, "first bad record ends the log") cannot: it truncates corrupted acknowledged records silently when they sit at the end of the log.

Un WAL de un solo nodo puede distinguir en el recovery una cola rota (un batch cuyo fsync nunca volvió) de la corrupción de registros confirmados, sin réplicas, usando evidencia redundante en cada sector de 512 bytes: CRC del sector, época del log, número de batch y la dirección del propio sector. La línea base (un CRC por registro, "el primer registro malo termina el log") no puede: trunca en silencio registros confirmados corruptos cuando están al final del log.

## Fault model / Modelo de fallas

Exactly one fault per crash (as in `prereg.toml`):

- **Torn tail (50%)**: the crash interrupts one more, never acknowledged batch. Each 512 B or 4 KiB write unit of that batch persists independently with p = 0.5. Sector writes are atomic.
- **Bit flip (25%)**: 1 to 8 bits in one acknowledged sector.
- **Misdirected write (25%)**: one acknowledged sector is overwritten with the image of a different acknowledged sector.
- Corruption faults hit the last acknowledged batch with p = 0.5 (the case the baseline gets wrong), otherwise a uniform acknowledged sector.
- 50% of trials start on a recycled log (valid sectors of an older epoch), 50% on a zero-filled one.

**Out of scope (declared before measuring):** lost writes (an acknowledged write that never reached the medium), zeroed sectors, faults that hit several sectors or the evidence and the data at once, tearing inside a 512-byte sector, real disks and fsync latency. Under those, the 100% detection claim does not hold and is not claimed.

## Metrics and thresholds / Métricas y umbrales

| Metric | Threshold |
|---|---|
| Corruptions of acknowledged data reported as corruption | 100% (0 silent truncations, 0 "clean") |
| Torn tails recovered automatically with every acknowledged record intact | >= 99% |
| Extra bytes written vs the baseline format, same batching and sector padding | <= 5% |
| Negative control: 10k fault-free trials (seeds 1000000..1009999) with any report | 0 |
| Positive control A: baseline heuristic on the same 100k trials silently truncates acknowledged records | > 0 expected, reported as measured |
| Positive control B: real `mmap-wal-log` (pinned at 739ec7e), 1000 trials flipping a bit in the last acknowledged record | > 0 silent losses expected, reported as measured |

Main run: seeds 0..99999 (100 000 crashes). If a result is 0 out of N, the README reports the rule-of-three 95% bound (3/N).
