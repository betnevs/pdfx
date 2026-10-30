import { act, renderHook, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { prepareTextBoxMovePreview, type TextBoxMovePreview, type TextBoxTargetView } from "./backend";
import { useTextMovePreview } from "./useTextMovePreview";
vi.mock("./backend", () => ({prepareTextBoxMovePreview: vi.fn()}));
it("does not reuse a pending background after the document revision changes", async () => {
  const prepare=vi.mocked(prepareTextBoxMovePreview);
  const pending: Array<(value: TextBoxMovePreview) => void>=[];
  prepare.mockImplementation(() => new Promise(resolve => pending.push(resolve)));
  const box={boxId: 1} as TextBoxTargetView;
  const payload=(pngBase64:string):TextBoxMovePreview => ({
    background:{documentId:"doc",pageIndex:0,scale:1,width:612,height:792,pngBase64},
    foreground:{documentId:"doc",pageIndex:0,scale:1,x:10,y:10,width:100,height:20,pngBase64},
  });
  const {result,rerender}=renderHook(({revision}) => useTextMovePreview("doc",0,box,revision,1),{initialProps:{revision:0}});
  await waitFor(()=>expect(prepare).toHaveBeenCalledTimes(1));
  rerender({revision:1});
  await waitFor(()=>expect(prepare).toHaveBeenCalledTimes(2));
  await act(async()=>{pending[0](payload("old"));});
  expect(result.current.status).toBe("loading");
  expect(result.current.data).toBeNull();
  await act(async()=>{pending[1](payload("new"));});
  expect(result.current.status).toBe("ready");
  expect(result.current.data?.background.pngBase64).toBe("new");
});
