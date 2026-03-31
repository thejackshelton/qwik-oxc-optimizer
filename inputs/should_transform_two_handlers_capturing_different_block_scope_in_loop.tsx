
import { component$, useSignal } from '@qwik.dev/core';

export default component$(() => {
  const arr = useSignal(['a', 'b'])
  return (
    <div>
      {arr.value.map((val, i) => {
        const index = i + 1;
        const label = val.toUpperCase();
        return (
          <div>
            <button onClick$={() => console.log(index)}>{val}</button>
            <span onKeyDown$={() => console.log(label)}>{label}</span>
          </div>
        )
      })}
    </div>
  );
});
