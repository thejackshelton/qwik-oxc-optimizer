
import { component$, useSignal } from '@qwik.dev/core';

export default component$(() => {
  const foo = useSignal('hi');
  const bar = useSignal('ho');
  const loop: string[] = ['abcd', 'xyz'];
  return (
    <div>
      {loop.map((item, index) => {
        return <div onClick$={() => console.log(item, foo.value)} onHover$={() => console.log(bar.value, index)}>{item}</div>
      })}
    </div>
  );
});

