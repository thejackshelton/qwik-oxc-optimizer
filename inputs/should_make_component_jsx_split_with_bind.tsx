
import { component$, useSignal } from '@qwik.dev/core';

export default component$(() => {
  const Cmp = Math.random() >= 0.5 ? 'button' : 'input'
  const sig = useSignal(0)
  return (
    <div>
      <Cmp bind:value={sig} />
    </div>
  );
});
