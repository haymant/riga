"""Sample Python: a tiny dataclass-backed summary tool."""
from __future__ import annotations

from dataclasses import dataclass, field
from statistics import mean


@dataclass
class Measurement:
    label: str
    values: list[float] = field(default_factory=list)

    @property
    def average(self) -> float:
        return mean(self.values) if self.values else 0.0


def summarize(measurements: list[Measurement]) -> dict[str, float]:
    """Return a label -> rounded average mapping."""
    return {m.label: round(m.average, 2) for m in measurements}


def main() -> None:
    samples = [
        Measurement("cpu", [12.0, 18.5, 9.25]),
        Measurement("mem", [2048.0, 4096.0]),
    ]
    for label, value in summarize(samples).items():
        print(f"{label}: {value}")


if __name__ == "__main__":
    main()
