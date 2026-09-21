# BitBlob

A smooth, rounded lavender companion with dark oval eyes and a soft ball antenna, inspired by the user-provided visual reference. The artwork uses a clear 3D toy style, with no pixel-art conversion.

## Asset contract

- `pet.json`: id `bitblob`, display name `BitBlob`, `spriteVersionNumber: 2`.
- `spritesheet.webp`: transparent, lossless WebP; 1536 × 2288 pixels; 8 columns × 11 rows; 192 × 208 pixels per cell.
- Rows 0–8: idle (6), running-right (8), running-left (8), waving (4), jumping (5), failed (8), waiting (6), running (6), review (6). The v2 neutral/default pose occupies row 0, column 6; all other unused cells are transparent.
- Row 9: 000, 022.5, 045, 067.5, 090, 112.5, 135, 157.5 degrees.
- Row 10: 180, 202.5, 225, 247.5, 270, 292.5, 315, 337.5 degrees.
- Directions are clockwise in screen coordinates: 000 up, 090 right, 180 down, 270 left. Neutral remains the idle frame.

## Generation and prompt summary

Created with the built-in image generation tool and the `hatch-pet` v2 workflow on 2026-09-21. The canonical prompt specifies a compact lavender blob, dark purple inset oval eyes, a short soft antenna with purple ball, optional small attached hands, smooth soft shading, no mouth or props, no text, shadows or detached effects. The supplied sheet serves only as a visual reference.

Each state was generated as its own grounded strip: quiet breathing/blinking; rightward squash/stretch gliding; connected-hand wave; squash/rise/peak/fall/landing; disappointed antenna droop; expectant hands-together asking pose; focused stationary task processing; calm thoughtful review. Leftward gliding is the sole derived visual row, produced by an approved framewise mirror of the symmetric rightward sequence without reversing timing. The four-cardinal anchors and each eight-pose look row were generated separately. Look directions keep the lower base planted while the upper face and inset eyes turn/stretch toward the target, with gentle antenna follow-through.

Deterministic extraction, registration, composition and edge cleanup use the skill's scripts. Jumping uses the supported shared-viewport extraction to preserve the generated vertical arc; other standard rows use connected-component extraction.

## Verification

Validation covers the v2 geometry, populated and unused cells, alpha/chroma cleanup, frame clipping and component extraction. Independent visual review covers all nine animations, every look direction and continuity around the row boundaries. Three isolated reviewers classify the randomized horizontal/vertical direction sheet; cardinal ambiguity blocks packaging. Runtime/transport integration checks belong to the companion feature tests rather than the image pipeline.

The completed run passed v2 atlas validation and a single alpha-preserving edge cleanup, with no atlas errors or warnings. All four cardinal directions passed three-reviewer blind consensus. The subtle horizontal component at 22.5° remains a reviewed warning; labeled review confirms the intended quadrant. The closing upper-left-to-up step and two continuity metrics also remain reviewed visual warnings, without a wrong quadrant, clipping or visible positional snap.
