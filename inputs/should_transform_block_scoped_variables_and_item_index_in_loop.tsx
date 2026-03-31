
import { component$, useSignal } from '@qwik.dev/core';

export default component$(() => {
  const arr = useSignal(['a', 'b'])
  return (
    <div>
      {arr.value.map((val, i) => {
        const index = i+1;
        return <div onClick$={() => console.log(val, i, index)}>{val}</div>
      })}
    </div>
  );
});
