export function revealTimelineInterval(second: number, resolution: number, maxSecond: number, windowSize: number, start: number, end: number) {
  second = Math.floor(Math.max(0, Math.min(second, maxSecond)) / resolution) * resolution;
  const limit = Math.max(0, maxSecond - windowSize);
  let nextStart = start;
  if (second < start) nextStart = second;
  else if (second + resolution > end) nextStart = Math.min(maxSecond, second + resolution) - windowSize;
  nextStart = Math.max(0, Math.min(limit, nextStart));
  return { second, offset: limit - nextStart };
}
