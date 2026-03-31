
import { component$ } from '@qwik.dev/core';

export default component$(() => {
  const loop: string[] = ['abcd', 'xyz'];
  return (
    <div>
      {loop.map((item, index) => {
        return <div onClick$={() => console.log(item)} onHover$={() => console.log(index)}>{item}</div>
      })}
    </div>
  );
});
