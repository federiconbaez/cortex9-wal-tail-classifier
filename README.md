# cortex9-wal-tail-classifier

Single-node WAL whose recovery tells a torn tail from corruption of committed records.

> **ES** abajo · **EN** below

## ES

### Qué es

Al reabrir un WAL después de un corte, un registro con checksum malo al final puede ser una escritura a medio terminar (cola rota, se trunca) o un registro ya confirmado al cliente que se corrompió después (truncarlo pierde datos en silencio). Con un CRC por registro no se pueden distinguir. RocksDB lo dice explícitamente para `kTolerateCorruptedTailRecords`, y [`mmap-wal-log`](https://github.com/federiconbaez/mmap-wal-log) pone a cero en silencio un último registro corrupto.

Este WAL agrega evidencia redundante y barata en cada sector de 512 bytes: `crc32c | época | batch | dirección`. El recovery aplica tres reglas:

- Un CRC malo, o un sector válido que dice ser de otra dirección, no lo produce una escritura interrumpida (con escrituras de sector atómicas): es corrupción, se reporta y no se trunca.
- Un batch (la unidad de un fsync) incompleto es cola rota solo si no hay ningún sector de un batch posterior. El batch `b+1` se escribe después de que volvió el fsync de `b`.
- Los sectores de una época anterior (log reciclado) o en cero son fin de log.

### Pre-registro

Modelo de fallas, workload, semillas y umbrales en [PREREG.md](PREREG.md) y [prereg.toml](prereg.toml):

- Commit `16ff5f9`, publicado el 2026-10-29 a las 00:09:44 ART, antes de escribir el código y antes de medir.
- sha256 de `prereg.toml`: `3aa707131ee491966d712af0983de0dbb7d4cf0f4e61fb244e8adf77d3e8707e`.
- El binario compila `prereg.toml` adentro y toma de ahí los umbrales.
- La depuración usó semillas 9 000 000 a 9 001 999, fuera de los rangos registrados. Ahí apareció y se corrigió un bug de padding en la línea base: un header que cruzaba un sector se confundía con relleno.
- Desvíos del protocolo: 12 intentos de "escritura mal dirigida" no eran aplicables en el formato clasificador (13 en la línea base) porque el log confirmado tenía un solo sector y no había otro sector confirmado para copiar. Se cuentan aparte y no entran en los denominadores. El pre-registro no fijaba este caso.

### Resultados (corrida registrada)

Máquina: VM Linux 6.12, 8 vCPU Intel Xeon, rustc 1.99.1, release, 4 hilos. Binario del commit `372b889`.

| Criterio pre-registrado | Comando | Resultado | Estado |
|---|---|---|---|
| 100% de corrupciones confirmadas detectadas | `wal-tail-classifier main` (semillas 0..99 999) | 49 855 / 49 855 detectadas, 0 pérdidas silenciosas | PASS |
| ≥ 99% de colas rotas recuperadas solas | ídem | 50 133 / 50 133 (100%) | PASS |
| Overhead ≤ 5% en bytes contra la línea base | ídem | +2,07% (17 058 597 376 contra 16 712 350 720 bytes) | PASS |
| Control negativo: 0 reportes sin fallas | `wal-tail-classifier negative` (semillas 1 000 000..1 009 999) | 0 de 10 000 | PASS |
| Control positivo A: la línea base pierde datos | `main` | 25 462 pérdidas silenciosas de registros confirmados; detecta solo 48,19% de las corrupciones; 1 318 colas rotas reportadas como corrupción | esperado, PASS |
| Control positivo B: `mmap-wal-log` real (739ec7e) | `wal-tail-classifier mmap-control` | 1000 / 1000 últimos registros corruptos truncados en silencio | esperado, PASS |

**Cotas para los resultados en 0 (regla del tres, 95%):**

- Tasa de corrupciones confirmadas no detectadas < 3/49 855 ≈ 0,006%.
- Tasa de colas rotas no recuperadas < 3/50 133 ≈ 0,006%.
- Tasa de reportes falsos sin fallas < 3/10 000 = 0,03%.

Todas valen solo dentro del modelo de fallas.

Respecto del payload, el formato clasificador escribe +5,80% y la línea base +3,65%, contando relleno de sector y headers. El criterio pre-registrado es contra la línea base.

### Qué no prueba (honesto)

- El 100% es consecuencia del modelo: una falla por crash, escrituras de sector atómicas, sin pérdida de escrituras, sin sectores puestos en cero y sin fallas que toquen a la vez datos y evidencia. Fuera de eso hay casos indistinguibles y no se afirma nada. Lo que mide el experimento es que la implementación cumple el modelo y que la línea base no.
- No hay disco real: el p99 de commit no está medido.
- El número de batch es de 16 bits, sin manejo de vuelta (la prueba usa ≤ 33 batches).

### Uso

```sh
cargo build --release
./target/release/wal-tail-classifier main --threads 4
./target/release/wal-tail-classifier negative --threads 4
./target/release/wal-tail-classifier mmap-control
```

### Licencia

Apache-2.0. Ver [LICENSE](LICENSE).

## EN

### What it is

When a WAL reopens after a power cut, a record with a bad checksum at the end can be a half-finished write (a torn tail, to be truncated) or a record already acknowledged to the client that got corrupted later (truncating it silently loses data). One CRC per record cannot tell them apart. RocksDB says so for `kTolerateCorruptedTailRecords`, and [`mmap-wal-log`](https://github.com/federiconbaez/mmap-wal-log) silently zeroes a corrupted last record.

This WAL adds cheap redundant evidence to every 512-byte sector: `crc32c | epoch | batch | address`. Recovery applies three rules:

- A bad CRC, or a valid sector claiming another address, cannot come from an interrupted write (with atomic sector writes): it is corruption, reported, never truncated.
- An incomplete batch (one fsync's worth) is a torn tail only if no sector of a later batch exists. Batch `b+1` is written after `b`'s fsync returned.
- Sectors of an older epoch (a recycled log) or all-zero sectors mark the end of the log.

### Pre-registration

Fault model, workload, seeds and thresholds are in [PREREG.md](PREREG.md) and [prereg.toml](prereg.toml):

- Commit `16ff5f9`, pushed on 2026-10-29 at 00:09:44 ART, before the code was written and before measuring.
- sha256 of `prereg.toml`: `3aa707131ee491966d712af0983de0dbb7d4cf0f4e61fb244e8adf77d3e8707e`.
- The binary compiles `prereg.toml` in and takes its thresholds from it.
- Debugging used seeds 9,000,000 to 9,001,999, outside the registered ranges. A baseline padding bug showed up there and was fixed: a record header crossing a sector was mistaken for padding.
- Protocol deviations: 12 misdirected-write trials were not applicable in the classifier format (13 in the baseline) because the acknowledged log had a single sector and there was no other acknowledged sector to copy. They are counted separately and left out of the denominators. The pre-registration did not fix this case.

### Results (registered run)

Machine: Linux 6.12 VM, 8 vCPU Intel Xeon, rustc 1.99.1, release, 4 threads. Binary from commit `372b889`.

| Pre-registered criterion | Command | Result | Status |
|---|---|---|---|
| 100% of acknowledged-data corruptions detected | `wal-tail-classifier main` (seeds 0..99,999) | 49,855 / 49,855 detected, 0 silent losses | PASS |
| ≥ 99% of torn tails recovered automatically | same | 50,133 / 50,133 (100%) | PASS |
| Bytes overhead ≤ 5% vs baseline | same | +2.07% (17,058,597,376 vs 16,712,350,720 bytes) | PASS |
| Negative control: 0 reports without faults | `wal-tail-classifier negative` (seeds 1,000,000..1,009,999) | 0 of 10,000 | PASS |
| Positive control A: the baseline loses data | `main` | 25,462 silent losses of acknowledged records; detects only 48.19% of corruptions; 1,318 torn tails reported as corruption | expected, PASS |
| Positive control B: real `mmap-wal-log` (739ec7e) | `wal-tail-classifier mmap-control` | 1000 / 1000 corrupted last records silently truncated | expected, PASS |

**Bounds for the zero results (rule of three, 95%):**

- Rate of undetected acknowledged-data corruption < 3/49,855 ≈ 0.006%.
- Rate of unrecovered torn tails < 3/50,133 ≈ 0.006%.
- Rate of false reports without faults < 3/10,000 = 0.03%.

All of them hold only inside the fault model.

Relative to payload, the classifier format writes +5.80% and the baseline +3.65%, counting sector padding and headers. The pre-registered criterion is against the baseline.

### What it does not show (honest)

- The 100% follows from the model: one fault per crash, atomic sector writes, no lost writes, no zeroed sectors, no fault hitting data and evidence at once. Outside it there are indistinguishable cases and nothing is claimed. What the experiment measures is that the implementation meets the model and the baseline does not.
- No real disk: commit p99 is not measured.
- The batch number is 16-bit with no wrap handling (trials use ≤ 33 batches).

### Usage

See the commands in the Spanish section.

### License

Apache-2.0. See [LICENSE](LICENSE).
