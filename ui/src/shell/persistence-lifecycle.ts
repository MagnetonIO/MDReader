/** A timeout is an unconfirmed save, never permission to close the window. */
export async function drainBeforeClose(pending: Promise<unknown>, timeoutMs = 2000): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      pending,
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error('Persistence did not settle')), timeoutMs);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}
