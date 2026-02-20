export interface LatestRequestGate {
  next: () => number;
  isLatest: (token: number) => boolean;
  invalidate: () => void;
}

export function createLatestRequestGate(): LatestRequestGate {
  let current = 0;

  return {
    next() {
      current += 1;
      return current;
    },
    isLatest(token: number) {
      return token === current;
    },
    invalidate() {
      current += 1;
    },
  };
}
