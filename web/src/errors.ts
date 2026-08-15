export function messageFrom(error: unknown): string {
  return error instanceof Error ? error.message : "请求失败，请稍后重试";
}
