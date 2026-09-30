/** Keep the source hidden while another Develop asset is opening. */
export function developPreviewPresentation(
  panelOpen: boolean,
  showOriginal: boolean,
  selectedFileId: number | null,
  activeFileId: number | null,
): { showOverlay: boolean; useCurrentFrame: boolean } {
  const hasSelection = selectedFileId !== null && selectedFileId > 0;
  const useCurrentFrame = hasSelection && selectedFileId === activeFileId;
  return {
    showOverlay:
      panelOpen && hasSelection && (!showOriginal || !useCurrentFrame),
    useCurrentFrame,
  };
}
