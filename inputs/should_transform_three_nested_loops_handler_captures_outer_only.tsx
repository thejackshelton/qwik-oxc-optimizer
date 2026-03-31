
import { component$, useSignal } from '@qwik.dev/core';

export default component$(() => {
  const data = useSignal([
    [['a', 'b'], ['c', 'd']],
    [['e', 'f'], ['g', 'h']],
  ]);
  return (
    <div>
      {data.value.map((plane, pi) => {
        const planeId = 'p' + pi;
        return (
          <div key={pi}>
            {plane.map((row, ri) => (
              <div key={ri}>
                {row.map((cell, ci) => (
                  <button key={ci} onClick$={() => console.log(planeId)}>
                    {cell}
                  </button>
                ))}
              </div>
            ))}
          </div>
        );
      })}
    </div>
  );
});
