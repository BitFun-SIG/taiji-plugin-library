import React from 'react';
import './ModelModeAnimation.scss';

interface ModelModeAnimationProps {
  mode: 'smart' | 'pool';
}

type StarStyle = React.CSSProperties & Record<`--_model-star-${string}`, string | number>;

// Stable, irregular clouds keep the material still across React renders.
function starCloud(count: number, width: number, height: number, phase: number) {
  return Array.from({ length: count }, (_, index) => {
    const angle = index * 2.399963 + phase + (index % 3) * 0.17;
    const spread = Math.sqrt((index + 0.5) / count);
    return {
      x: Math.cos(angle) * spread * width / 2,
      y: Math.sin(angle) * spread * height / 2,
      radius: 0.7 + (index % 4) * 0.2,
      opacity: 0.5 + (index % 5) * 0.08,
    };
  });
}

const sourceStars = starCloud(9, 30, 36, 0.7);
const destinationStars = starCloud(18, 45, 29, 1.1);
const travelingStars = starCloud(20, 46, 30, 2.3);
const energyStars = starCloud(52, 69, 83, 0.4);
const transferStars = starCloud(12, 38, 23, 1.8);
const routingRegions = [
  { side: 'upper', y: 29, edgeY: 17, alternateY: 101 },
  { side: 'lower', y: 101, edgeY: 111, alternateY: 29 },
] as const;
const energyRegions = [
  { side: 'first', x: 53 },
  { side: 'next', x: 227 },
] as const;

const px = (value: number) => `${value.toFixed(2)}px`;

/** Decorative density changes only; no routing lines, cell outlines or runtime state. */
export const ModelModeAnimation: React.FC<ModelModeAnimationProps> = ({ mode }) => (
  <svg
    className="openbitfun-model-mode"
    data-openbitfun-component="model-selector"
    data-openbitfun-part="modeMaterial"
    data-mode={mode}
    viewBox="0 0 280 128"
    preserveAspectRatio="none"
    aria-hidden="true"
    focusable="false"
  >
    {mode === 'smart' ? (
      <>
        {sourceStars.map((star, index) => (
          <circle
            key={index}
            className="openbitfun-model-mode__source"
            cx={34 + star.x}
            cy={64 + star.y}
            r={star.radius}
            style={{
              '--_model-star-opacity': star.opacity,
              '--_model-star-delay': `${-(index % 4) * 0.11}s`,
              '--_model-star-gather-x': px(-star.x * 0.42),
              '--_model-star-gather-y': px(-star.y * 0.42),
              '--_model-star-sway-x': px(star.y * 0.32),
              '--_model-star-sway-y': px(-star.x * 0.4),
            } as StarStyle}
          />
        ))}
        {routingRegions.map(({ side, y, edgeY, alternateY }) => (
          <g key={side} className={`openbitfun-model-mode__region openbitfun-model-mode__region--${side}`}>
            {destinationStars.map((star, index) => (
              <circle
                key={`destination-${index}`}
                className="openbitfun-model-mode__destination"
                cx={226 + star.x}
                cy={y + star.y}
                r={star.radius}
                style={{
                  '--_model-star-opacity': star.opacity,
                  '--_model-star-delay': `${-(index % 6) * 0.09}s`,
                  '--_model-star-gather-x': px(-star.x * 0.36),
                  '--_model-star-gather-y': px(-star.y * 0.36),
                  '--_model-star-spread-x': px(star.x * 0.4),
                  '--_model-star-spread-y': px(star.y * 0.22),
                  '--_model-star-sway-x': px(star.y * 0.4 - star.x * 0.12),
                  '--_model-star-sway-y': px(-star.x * 0.22),
                } as StarStyle}
              />
            ))}
            {travelingStars.map((star, index) => (
              <circle
                key={`travel-${index}`}
                className="openbitfun-model-mode__traveler"
                cx={37 + star.x * 0.55}
                cy={64 + star.y}
                r={star.radius}
                style={{
                  '--_model-star-opacity': star.opacity,
                  '--_model-star-delay': `${-(index % 5) * 0.12}s`,
                  '--_model-star-probe-x': px(52 + star.x * 0.15),
                  '--_model-star-probe-y': px(y - 64 - star.y * 0.5),
                  '--_model-star-mid-x': px(110 + star.x * 0.2),
                  '--_model-star-mid-y': px(edgeY - 64 - star.y * 0.6),
                  '--_model-star-end-x': px(189 + star.x * 0.09),
                  '--_model-star-end-y': px(y - 64 - star.y * 0.4),
                  '--_model-star-relay-x': px(206 - star.x * 0.2),
                  '--_model-star-relay-y': px(-star.y * 0.5),
                  '--_model-star-alternate-x': px(176 - star.x * 0.05),
                  '--_model-star-alternate-y': px(alternateY - 64 - star.y * 0.38),
                  '--_model-star-release-x': px(181 + star.x * 0.41),
                  '--_model-star-release-y': px(alternateY - 64 - star.y * 0.18),
                } as StarStyle}
              />
            ))}
          </g>
        ))}
      </>
    ) : energyRegions.map(({ side, x }) => (
      <g key={side} className={`openbitfun-model-mode__region openbitfun-model-mode__region--${side}`}>
        {energyStars.map((star, index) => (
          <circle
            key={`energy-${index}`}
            className="openbitfun-model-mode__energy"
            cx={x + star.x}
            cy={64 + star.y}
            r={star.radius}
            style={{
              '--_model-star-opacity': star.opacity,
              '--_model-star-delay': `${-(index % 7) * 0.09}s`,
              '--_model-star-seed-x': px(-star.x * 0.66),
              '--_model-star-seed-y': px(34 - star.y * 0.86),
              '--_model-star-gather-x': px(-star.x * 0.28),
              '--_model-star-gather-y': px(-star.y * 0.28),
              '--_model-star-sway-x': px(star.y * 0.22 - star.x * 0.08),
              '--_model-star-sway-y': px(-star.x * 0.3 - star.y * 0.12),
              '--_model-star-spread-x': px(star.x * 0.45),
              '--_model-star-spread-y': px(star.y * 0.16),
            } as StarStyle}
          />
        ))}
        {transferStars.map((star, index) => (
          <circle
            key={`transfer-${index}`}
            className="openbitfun-model-mode__transfer"
            cx={x + star.x * 0.65}
            cy={64 + star.y * 0.55}
            r={star.radius}
            style={{
              '--_model-star-opacity': star.opacity,
              '--_model-star-delay': `${-(index % 4) * 0.15}s`,
              // Leave each cloud before crossing the card above or below its label.
              '--_model-star-outlet-x': px((140 - x) * 0.3 - star.x * 0.25),
              '--_model-star-outlet-y': px((index % 3 === 0 ? -34 : 34) - star.y * 0.3),
              '--_model-star-mid-x': px(140 - x - star.x * 0.2),
              '--_model-star-mid-y': px((index % 3 === 0 ? -46 : 46) - star.y * 0.3),
              '--_model-star-inlet-x': px((140 - x) * 1.7 - star.x * 0.25),
              '--_model-star-inlet-y': px((index % 3 === 0 ? -30 : 30) - star.y * 0.3),
              '--_model-star-end-x': px(280 - x * 2),
              '--_model-star-end-y': px(26 - star.y * 0.3),
            } as StarStyle}
          />
        ))}
      </g>
    ))}
  </svg>
);
