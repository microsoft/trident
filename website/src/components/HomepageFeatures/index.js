import clsx from 'clsx';
import Heading from '@theme/Heading';
import styles from './styles.module.css';
const { sprintf } = require('sprintf-js');

const FeatureList = [
  {
    title: 'Install',
    // Svg: require('@site/static/img/trident_install.svg').default,
    description: (
      <>
        Trident is great for installing Azure Linux.
      </>
    ),
  },
  {
    title: 'Service ',
    // Svg: require('@site/static/img/trident_service.svg').default,
    description: (
      <>
        Trident helps you manage and update Azure Linux.
      </>
    ),
  },
];

function Feature({ title, description, featureRowClass }) {
  return (
    <div className={clsx(featureRowClass)}>
      <div className="text--center padding-horiz--md">
        <Heading as="h3">{title}</Heading>
        <p>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures() {
  // There seem to be 12 columns for the feature list ... 'col--6` says each
  // feature gets 6 columns, so 2 per row ... the group of 2 would be centered.
  // If there were 1 feature, it would not be centered, but would look left
  // justified ... 1 could be centered if `col--12` was used.
  //
  // Do some work here to try to keep things centered-ish for various counts of
  // features.
  let featureRowClass = 'col col--3';
  if (FeatureList.length > 0 && FeatureList.length < 4) {
    featureRowClass = sprintf("col col--%d", 12 / (FeatureList.length))
  }

  return (
    <section className={styles.features}>
      <div className="container">
        <div className="row">
          {FeatureList.map((props, idx) => (
            <Feature key={idx} {...props} featureRowClass={featureRowClass} />
          ))}
        </div>
      </div>
    </section>
  );
}
